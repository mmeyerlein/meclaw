//! GH #1440 (D38 Z3) -- under `summary_form` "brief" the `sources` of a
//! summary are wall rows, never a summary.
//!
//! WHY this file exists: a chained prose summary counts the summary before it
//! as a source (`transcript_of`, GH #925), so after two rebuilds `sources`
//! names a summary, and a pointer `[#<id>]` of a kept line would lead
//! `history_read` to a summary instead of the words that were said. The
//! brief keeps its old lines unchanged, so the new row's `sources` are the
//! old row's sources (wall rows) plus the rows that went in this time; the
//! old summary's hash is in none of them. The audience of the summary still
//! meets the old one's (GH #925) -- that rule is pinned in `curator_policy`.
//!
//! Stub summarizer (`support/curator_hive.rs` `Hive::answer`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};
use std::collections::BTreeSet;

fn sources_of(h: &Hive) -> Vec<BTreeSet<String>> {
    h.rows("SELECT sources FROM summaries ORDER BY rowid")
        .into_iter()
        .map(|r| {
            let v: Value =
                meclaw_core::serde_json::from_str(r[0].as_str().expect("sources")).expect("json");
            v.as_array()
                .expect("a list")
                .iter()
                .map(|x| x.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .collect()
}

fn column(h: &Hive, sql: &str) -> BTreeSet<String> {
    h.rows(sql)
        .into_iter()
        .map(|r| r[0].as_str().unwrap_or_default().to_string())
        .collect()
}

#[test]
fn sources_are_wall_rows_never_a_summary() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[
        ("policy", "keep_recent", json!(1)),
        ("policy", "summary_form", json!("brief")),
    ]);
    turn(&mut h, "s1", "t1", "q1", "a1", json!({}));
    turn(&mut h, "s1", "t2", "q2", "a2", json!({}));
    turn(
        &mut h,
        "s1",
        "t3",
        "q3",
        "a3",
        json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
    );
    h.fire(&last_add(&h));
    let q1 = short_id(&user("q1"));
    h.answer(
        &format!(
            "- person (2026-11-04) hannah/lives_in: Hannah lives in Lisbon. [asserted] [#{q1}]"
        ),
        "stop",
    );
    turn(
        &mut h,
        "s1",
        "t4",
        "q4",
        "a4",
        json!({"cache_expires_at": "2099-01-01T00:01:00Z"}),
    );
    h.fire(&last_add(&h));
    let q3 = short_id(&user("q3"));
    h.answer(
        &format!("- person (2026-11-05) garden/state: The garden is in bloom. [asserted] [#{q3}]"),
        "stop",
    );

    let all = sources_of(&h);
    assert_eq!(all.len(), 2, "two summaries: {:?}", h.stderr);
    let wall = column(&h, "SELECT hash FROM wall");
    let summaries = column(&h, "SELECT hash FROM summaries");
    for (i, s) in all.iter().enumerate() {
        assert!(
            s.is_subset(&wall),
            "summary {i}: every source is a wall row: {:?}",
            s.difference(&wall).collect::<Vec<_>>()
        );
        assert!(
            s.is_disjoint(&summaries),
            "summary {i}: no source is a summary"
        );
    }
    assert!(
        all[0].is_subset(&all[1]),
        "the kept lines keep their sources: {:?} vs {:?}",
        all[0],
        all[1]
    );
    assert!(
        all[1].len() > all[0].len(),
        "and the rows that went in this time join them"
    );
}
