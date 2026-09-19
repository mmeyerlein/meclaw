//! GH #708 -- the hold threshold is a token, and the debounce is not a clock.
//!
//! § 5.5 names the threshold in the description itself: "pressing the OS mark
//! shorter than the hold threshold -- a token, 250 ms". § 2 says where a
//! quantity lives: "a named quantity in the sheet … numbers stand only there,
//! never in curator code". The client read a JavaScript constant instead, so a
//! screen whose sheet said one thing and whose script said another had two
//! thresholds and no way to tell.
//!
//! The second half is § 3.2: "no state in the browser carries meaning, with one
//! exception: whether the dock is open or closed right now". The old debounce
//! held a 700 ms window (`CLICK_GRACE_MS`, `tapAt`) to decide whether a `click`
//! after a `pointerup` still belonged to the same gesture. Debouncing ONE
//! physical gesture is not meaning -- but a number in the browser that stands
//! in no sheet is exactly what § 2 forbids. So the debounce asks which way the
//! gesture already came, not how long ago.
//!
//! Skips when the templates do not ship (R2b).

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const SHEET: &str = "templates/display/compose/display-dna.css";
const COMPOSE: &str = "templates/display/compose/compose.py";

fn library_ships() -> bool {
    repo("templates/display/template.json").is_file()
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// One `NAME = (` … `\n)` block of the curator, as text. Asked of the block and
/// never of the whole file: `compose.py` carries the sheet as well as the two
/// scripts, and a needle against the file would find the answer in the wrong
/// half.
fn block(name: &str) -> String {
    let src = read(COMPOSE);
    let open = format!("{name} = (");
    let start = src.find(&open).unwrap_or_else(|| panic!("{name} is gone"));
    let end = src[start..]
        .find("\n)")
        .unwrap_or_else(|| panic!("{name} is not one parenthesised block"));
    src[start..start + end].to_string()
}

#[test]
fn the_threshold_comes_out_of_the_sheet() {
    if !library_ships() {
        return;
    }
    let os = block("OS_CLIENT_JS");
    assert!(
        !os.contains("HOLD_MS = 250"),
        "the threshold is still a number in the script -- § 5.5 calls it a \
         token and § 2 says numbers stand in the sheet"
    );
    assert!(
        os.contains("dur(cols") && os.contains("--hold-ms"),
        "the mark's hook does not read `--hold-ms` off the screen: the sheet \
         is where the quantity lives (§ 2 `Token`)"
    );
    let sheet = read(SHEET);
    assert_eq!(
        sheet.matches("--hold-ms: 250ms").count(),
        1,
        "`--hold-ms: 250ms` stands once in {SHEET}, or it is two thresholds \
         again (development-rules § 2d)"
    );
}

#[test]
fn the_debounce_is_a_gesture_and_not_a_window_of_time() {
    if !library_ships() {
        return;
    }
    let os = block("OS_CLIENT_JS");
    for gone in ["CLICK_GRACE_MS", "tapAt", "lastUpAt"] {
        assert!(
            !os.contains(gone),
            "`{gone}` is still in the mark's hook: a span of time in the \
             browser is a number outside the sheet (§ 2) and a state that \
             decides (§ 3.2). Ask which way the gesture already came instead."
        );
    }
    assert!(
        os.contains("gesture = \\\"pointer\\\"") || os.contains("gesture = \"pointer\""),
        "the hook does not record that a `pointerup` already ended this \
         gesture -- without it a Safari `click` after a touch switches the \
         dock a second time"
    );
    assert!(
        os.contains("gesture !== "),
        "and nothing asks it before switching on a `click`"
    );
}
