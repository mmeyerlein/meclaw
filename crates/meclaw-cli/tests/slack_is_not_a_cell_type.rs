//! P12 T-REG-3: Slack is instance TWO of the `proxy` cell type, not a new one.
//!
//! The whole shape of P12 rests on this: a second platform is a parameter of an
//! existing cell type, not another entry in the registry. If this count moves,
//! someone added a cell type instead of a platform variant, and the seam has
//! been bypassed rather than used.

//! The count below is a RATCHET, not a constant: it moves only when a cell
//! type is deliberately added, and moving it is the moment somebody has to
//! justify the addition in a commit. It went 13 → 14 with the `vault` type
//! (GH #151), which earns its own entry precisely because its guarantee — a
//! route surface with no read on it — cannot be a parameter of an existing
//! type. A platform variant can; a missing operation cannot.

use meclaw_cli::factories::built_in_factories;

#[test]
fn slack_did_not_add_a_cell_type() {
    let reg = built_in_factories();
    assert_eq!(
        reg.len(),
        14,
        "a cell type was added or removed — if that was deliberate, say why here; \
         a chat platform in particular is params.platform on proxy, never a type"
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
