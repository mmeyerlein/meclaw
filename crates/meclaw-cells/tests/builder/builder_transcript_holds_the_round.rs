//! The loop's memory. An `llm` cell forgets the conversation after every call
//! (`crates/meclaw-cells/src/llm/output.rs`), and a `code` cell has no cell.db
//! (`docs/cell-types.md` § code), so the round table is a `store` or it does
//! not exist. Five columns, and each one earns its place.

use meclaw_core::serde_json::Value;
use std::path::PathBuf;

fn transcript() -> Value {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../templates/builder/transcript/config.json");
    meclaw_core::serde_json::from_str(&std::fs::read_to_string(p).expect("transcript config"))
        .expect("parses")
}

#[test]
fn the_transcript_is_a_store_with_the_five_columns_a_round_needs() {
    let cfg = transcript();
    assert_eq!(cfg["cell"]["type"], "store");
    let cols = cfg["params"]["schema"]["thread"]
        .as_object()
        .expect("thread table declared");
    for col in ["build_id", "iter", "role", "turn", "fired", "recorded_at"] {
        assert!(
            cols.contains_key(col),
            "the thread table needs a {col} column"
        );
    }
}

/// The column types are not decoration. `StoreParams::parse`
/// (`crates/meclaw-cells/src/store/params.rs`) allows exactly `text`, `int` and
/// `json`, lower case, and refuses everything else at spawn — a `"TEXT"` in
/// this file would be a colony that does not boot, and no test above would have
/// noticed, because both of them only read the column NAMES. So this one asks
/// the parser instead of the file.
#[test]
fn the_declared_round_table_is_one_the_store_actually_accepts() {
    let cfg = transcript();
    meclaw_cells::store::StoreParams::parse(&cfg["params"])
        .expect("the transcript's params are a store declaration the substrate accepts");
}

#[test]
fn the_transcript_writes_only_from_inside() {
    let cfg = transcript();
    assert_eq!(
        cfg["contract"]["write_surface"], "internal",
        "nothing outside the builder may write the round table"
    );
}
