//! GH #1444 -- the curator's `rebuild` figure carries what the rebuilds cost:
//! `cost_cents_sum`, the cents of the `curate` marks whose `curate_mark` is
//! not `none` (a call that rebuilt), beside `n`, `by_mark` and `rate_pct`. The
//! watcher reads it as the role figure `rebuild_cost`. A mark without a cost
//! adds nothing; a call that did not rebuild adds nothing.
//!
//! The hive in one process (`support/curator_hive.rs`): the shipped script
//! under python3, the shipped edges, the ledger an in-memory SQLite.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::{Hive, shipped};
use meclaw_core::serde_json::json;

const SESSION: &str = "+15550100042-2026-11-01T08:00:00.000000Z";

#[test]
fn gh_d38_w1_the_rebuild_figure_sums_its_cost() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let marks = [
        (json!("none"), json!(5)),
        (json!("target"), json!(7)),
        (json!("hard"), json!(11)),
        (json!("cost"), json!(null)),
        (json!(null), json!(13)),
    ];
    for (i, (mark, cost)) in marks.iter().enumerate() {
        let v = json!({"call": "c", "curate_mark": mark, "window_tokens": 1000,
                       "cached_tokens": 900, "cost_cents": cost});
        h.db.execute(
            "INSERT INTO marks (seq, session_id, turn_id, kind, value, at) \
             VALUES (?1, ?2, ?3, 'curate', ?4, ?5)",
            rusqlite::params![
                i as i64 + 1,
                SESSION,
                format!("{SESSION}#{i}"),
                v.to_string(),
                format!("2026-11-01T0{}:00:00.000000Z", i + 1)
            ],
        )
        .expect("a marks row");
    }
    h.lane(
        "in_stats",
        json!({}),
        json!({"stats_tag": "w1", "stats_from": "2026-11-01T00:00:00Z",
               "stats_to": "2026-11-02T00:00:00Z", "stats_kinds": r#"["rebuild"]"#,
               "stats_samples": 0}),
        json!({"messages": []}),
    );
    assert_eq!(h.out.len(), 1, "one answer: {:?}", h.out);
    let m = &h.out[0];
    assert_eq!(m.hop["error_code"], "", "{:?}", h.stderr);
    let rebuild = &m.body["stats"]["counts"]["rebuild"];
    // target 7 + hard 11; `none` and the unmarked call did not rebuild, and
    // the `cost` rebuild without a cost adds nothing.
    assert_eq!(rebuild["cost_cents_sum"], json!(18), "{rebuild}");
    assert_eq!(rebuild["n"], json!(5), "{rebuild}");
}
