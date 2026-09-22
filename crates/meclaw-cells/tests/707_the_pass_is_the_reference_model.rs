//! display-hive.md § 0.7: the pass is code, and the code in the cell is the code in the
//! document. `compose.py` carries the twelve steps of `model/pass.py` verbatim between two
//! marker lines; this test compares the section with the travelling copy byte for byte
//! (the copy's own drift lock against the description tree is 710_…). A diff is a defect.
use std::fs;

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const BEGIN: &str = "# --- The pass: display-hive.md § 4, model/pass.py verbatim (begin) ---";
const END: &str = "# --- The pass: display-hive.md § 4, model/pass.py verbatim (end) ---";

#[test]
fn the_pass_in_the_cell_is_the_reference_model() {
    let cell = fs::read_to_string(repo("templates/display/compose/compose.py")).unwrap();
    let model = fs::read_to_string(repo("templates/display/compose/scenarios/pass.py")).unwrap();
    let start = cell.find(BEGIN).expect("begin marker") + BEGIN.len();
    let stop = cell.find(END).expect("end marker");
    let section = cell[start..stop].trim();
    // The model's head (module docstring + `import copy`) is not repeated in the cell;
    // everything from the first constant on is.
    let body = model[model.find("RUNGS = (").expect("the model's first constant")..].trim();
    assert_eq!(
        section, body,
        "compose.py's pass section differs from pass.py (§ 0.7)"
    );
}
