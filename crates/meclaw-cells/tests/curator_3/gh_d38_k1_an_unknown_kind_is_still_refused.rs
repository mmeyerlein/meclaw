//! GH #1426 -- the three new kinds of the curator's `stats` door (`window`,
//! `rebuild`, `gc`) widen what it answers, not what it accepts: a kind outside
//! the list -- among them `curate`, the mark the figures are read from -- is
//! still refused with `invalid_input` and nothing is read.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::{Hive, shipped};
use meclaw_core::serde_json::json;

#[test]
fn gh_d38_k1_an_unknown_kind_is_still_refused() {
    if !shipped() {
        return;
    }
    for kinds in [
        r#"["window","curate"]"#,
        r#"["gc","messages"]"#,
        r#"["rebuild","windows"]"#,
    ] {
        let mut h = Hive::new();
        h.lane(
            "in_stats",
            json!({}),
            json!({"stats_tag": "u", "stats_from": "2026-11-01T00:00:00Z",
                   "stats_to": "2026-11-02T00:00:00Z", "stats_kinds": kinds}),
            json!({"messages": []}),
        );
        assert_eq!(h.out.len(), 1, "{kinds}: {:?}", h.out);
        let m = &h.out[0];
        assert_eq!(m.hop["error_code"], "invalid_input", "{kinds}");
        assert_eq!(m.hop["detail"], "stats_kinds", "{kinds}");
        assert_eq!(m.body["stats"]["counts"], json!({}), "{kinds}");
        assert!(h.ledger_ops.is_empty(), "{kinds}: nothing read");
    }
    // The three new kinds alone are a question that holds.
    let mut h = Hive::new();
    h.lane(
        "in_stats",
        json!({}),
        json!({"stats_tag": "k", "stats_from": "2026-11-01T00:00:00Z",
               "stats_to": "2026-11-02T00:00:00Z",
               "stats_kinds": r#"["window","rebuild","gc"]"#}),
        json!({"messages": []}),
    );
    assert_eq!(h.out.len(), 1, "{:?}", h.out);
    assert_eq!(h.out[0].hop["error_code"], "", "{:?}", h.stderr);
}
