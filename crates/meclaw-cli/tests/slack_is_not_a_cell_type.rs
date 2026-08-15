//! P12 T-REG-3: Slack is instance TWO of the `proxy` cell type, not a new one.
//!
//! The whole shape of P12 rests on this: a second platform is a parameter of an
//! existing cell type, not another entry in the registry. If this count moves,
//! someone added a cell type instead of a platform variant, and the seam has
//! been bypassed rather than used.

use meclaw_cli::factories::built_in_factories;

#[test]
fn slack_did_not_add_a_cell_type() {
    let reg = built_in_factories();
    assert_eq!(
        reg.len(),
        13,
        "P12 must not change the number of built-in cell types"
    );
    assert!(
        reg.contains_key("proxy"),
        "the proxy cell type is still the bridge for every chat platform"
    );
    assert!(
        !reg.contains_key("slack"),
        "slack must NOT be its own cell type — it is params.platform on proxy"
    );
}
