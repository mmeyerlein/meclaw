//! GH #741 (2) — in the weather window the unit stands UNDER the number.
//!
//! The owner, acceptance round 2 (18.09.2026): the unit is to stand under the
//! value, not beside and raised behind it. The dock tile already draws it that
//! way — `TILE_TEMPLATE` lays `value` and `unit` out as siblings in a grid — and
//! the window was the exception: `.display-weather-unit` carried
//! `margin-inline-start` and `vertical-align: super`.
//!
//! § 9.1 says "tile with `value` and `unit` separate, window with a large
//! value"; where the unit stands in the window it does not say, so this is a
//! rendering decision and the sheet is the only place it is made. Value and
//! unit travel apart all the way from the ambient application, so nothing
//! outside the sheet moves (`messungen/G-rendering.md` § e).
//!
//! A file-text lock. Skips when the templates do not ship (R2b).

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const SHEET: &str = "templates/display/compose/display-dna.css";

fn library_ships() -> bool {
    repo("templates/display/template.json").is_file()
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// The declarations of the first rule whose selector list carries `sel`.
fn rule(sheet: &str, sel: &str) -> String {
    let at = sheet
        .find(sel)
        .unwrap_or_else(|| panic!("no rule carries the selector `{sel}`"));
    let open = sheet[at..]
        .find('{')
        .unwrap_or_else(|| panic!("`{sel}` opens no block"));
    let close = sheet[at + open..]
        .find('}')
        .unwrap_or_else(|| panic!("`{sel}` never closes"));
    sheet[at + open + 1..at + open + close].to_string()
}

/// The value is a column, and the unit is its second row.
#[test]
fn the_temperature_is_a_column_with_the_unit_below_it() {
    if !library_ships() {
        return;
    }
    let sheet = read(SHEET);
    let temp = rule(&sheet, ".display-weather-temp {");
    assert!(
        temp.contains("flex-direction: column"),
        "the number and its unit still stand on one line: {temp}"
    );
    let unit = rule(&sheet, ".display-weather-unit {");
    for beside in ["margin-inline-start", "vertical-align"] {
        assert!(
            !unit.contains(beside),
            "`{beside}` sets the unit beside the number, which is the thing \
             that changed (#741): {unit}"
        );
    }
}
