//! GH #1421 (M7-A) -- `via` names the third party a `reported` fact rests on,
//! and only a `reported` fact has one.
//!
//! A `via` beside any other stance (or beside none) falls away on its own --
//! the fact is filed without it -- and the block gets a
//! `via_without_reported` receipt. A `via` beside `reported` is kept as the
//! model wrote it (trimmed only: the name of the source is entity-faithful,
//! never case-folded). Bare script runs, no colony, no model.

// harness_fold: #[path = "support/memory_hive_subject.rs"]
#[allow(unused_imports)]
use crate::support; // harness_fold: mod support;

use meclaw_core::serde_json::json;
use support::*;

#[test]
fn a_via_beside_another_stance_falls_away_with_a_receipt() {
    if !shipped() {
        return;
    }
    let out = m7a_arrive(
        json!([
            m7a_fact(
                "harmless",
                json!({"stance": "believed", "via": "the paper"})
            ),
            m7a_fact("small", json!({"via": "the neighbour"})),
        ]),
        false,
    );
    let staged = m7a_staged(&out);
    assert_eq!(staged.len(), 2, "both facts are filed: {out:?}");
    for f in &staged {
        assert!(f.get("via").is_none(), "{f}");
    }
    assert_eq!(staged[0]["stance"], "believed", "{staged:?}");
    assert!(staged[1].get("stance").is_none(), "{staged:?}");
    // The fact that lost its `via` and names no stance is the old road again.
    let plain = m7a_staged(&m7a_arrive(json!([m7a_fact("small", json!({}))]), false));
    assert_eq!(staged[1]["claim_hash"], plain[0]["claim_hash"]);

    let rejects = m7a_rejects(&out);
    let lost: Vec<_> = rejects
        .iter()
        .filter(|(r, _)| r == "via_without_reported")
        .collect();
    assert_eq!(lost.len(), 1, "{rejects:?}");
    assert!(lost[0].1.contains("2 fact(s)"), "{lost:?}");
    assert!(
        !lost[0].1.contains("neighbour"),
        "a receipt counts, it never echoes: {lost:?}"
    );

    let rows = m7a_fact_rows(&m7a_apply(&staged, "", None));
    assert_eq!(rows.len(), 2, "{rows:?}");
    for r in &rows {
        assert!(r.get("via").is_none(), "{r}");
    }
}

#[test]
fn a_via_beside_reported_is_kept_as_written() {
    if !shipped() {
        return;
    }
    let out = m7a_arrive(
        json!([m7a_fact(
            "benign",
            json!({"stance": "reported", "via": "  Ms K. Next Door "})
        )]),
        false,
    );
    assert!(m7a_rejects(&out).is_empty(), "{out:?}");
    let staged = m7a_staged(&out);
    assert_eq!(staged[0]["via"], "Ms K. Next Door", "{staged:?}");
    let rows = m7a_fact_rows(&m7a_apply(&staged, "", None));
    assert_eq!(rows[0]["stance"], "reported");
    assert_eq!(rows[0]["via"], "Ms K. Next Door");
}

#[test]
fn an_empty_via_is_no_via_and_no_receipt() {
    if !shipped() {
        return;
    }
    let out = m7a_arrive(
        json!([
            m7a_fact("small", json!({"via": ""})),
            m7a_fact("round", json!({"via": null}))
        ]),
        false,
    );
    assert!(m7a_rejects(&out).is_empty(), "{out:?}");
    assert_eq!(m7a_staged(&out).len(), 2);
}
