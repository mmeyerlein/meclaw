//! The scenarios of the display hive against the real curator (display-hive.md § 11).
//!
//! The driver is Python and runs `compose.py`; this test runs the driver, so that the
//! pins stand in the nextest filter and every diff under `templates/display` pulls them
//! (`docs/development-rules.md` § 10). Three numbers have to be full: `MODEL` is the
//! document against itself (`run_model.py` against `pass.py`), `PURE` the scenarios
//! against the verbatim copy of the pass inside the cell, `CURATOR` the scenarios
//! through the cell's real entry -- two of them are colony-only and the driver says so
//! itself, which is why the third denominator is the smaller one.
//!
//! Both tests SKIP rather than fail where their material is legitimately absent: the
//! first when the library does not travel (R2b, the guard every tool-bound test in this
//! tree uses), the second when the private description is not there at all -- a foreign
//! clone and ci have the copies but not their source.

use std::process::Command;

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn home() -> String {
    std::env::var("HOME").unwrap_or_default()
}

const SCENARIOS: &str = "templates/display/compose/scenarios";
const DRIVER: &str = "templates/display/compose/scenarios/run_display_scenarios.py";

fn library_ships() -> bool {
    repo(DRIVER).exists()
}

/// `MODEL 109/109` -> `(109, 109)`. The driver writes the counter first and the
/// denominator second on a line of its own, one line per stage.
fn line(stdout: &str, prefix: &str) -> (u32, u32) {
    let rest = stdout
        .lines()
        .find_map(|l| l.strip_prefix(prefix))
        .unwrap_or_else(|| panic!("the driver printed no `{prefix}` line:\n{stdout}"));
    let (num, den) = rest
        .split_whitespace()
        .next()
        .and_then(|w| w.split_once('/'))
        .unwrap_or_else(|| panic!("`{prefix}` is no counter: {rest:?}"));
    (
        num.parse().expect("the counter"),
        den.parse().expect("the denominator"),
    )
}

#[test]
fn the_scenarios_run_against_the_curator() {
    if !library_ships() {
        return;
    }
    let out = Command::new("python3")
        .arg(repo(DRIVER))
        .output()
        .expect("the driver runs");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        out.status.success(),
        "the driver failed:\n{stdout}\n{stderr}"
    );
    for stage in ["MODEL ", "PURE ", "CURATOR "] {
        let (passed, total) = line(&stdout, stage);
        assert!(total > 0, "{stage}ran nothing:\n{stdout}");
        assert_eq!(passed, total, "{stage}is not full:\n{stdout}");
    }
}

/// The drift lock: the copy in the repo is byte for byte the description's model.
///
/// Without the private tree (ci, a foreign clone) SKIP -- the same guard the WebKit
/// laboratory uses. The three copies are compared as whole files: they are taken over
/// unchanged, `sync_pass.py` only carries `pass.py` on into `compose.py` and never edits
/// it, so nothing here may differ -- not even a docstring.
#[test]
fn the_copy_is_the_document() {
    let src = std::env::var("MECLAW_DISPLAY_HIVE")
        .unwrap_or_else(|_| format!("{}/projeks/MeClaw/meclaw-next/23-display", home()));
    if !std::path::Path::new(&src).is_dir() || !library_ships() {
        return;
    }
    for name in ["scenarios.json", "pass.py", "run_model.py"] {
        // The driver's own name for the model's runner; the other two keep theirs.
        let source_name = if name == "run_model.py" {
            "run.py"
        } else {
            name
        };
        let there = std::fs::read(format!("{src}/model/{source_name}"))
            .unwrap_or_else(|e| panic!("the source {source_name}: {e}"));
        let here = std::fs::read(repo(&format!("{SCENARIOS}/{name}")))
            .unwrap_or_else(|e| panic!("the copy {name}: {e}"));
        // `assert!`, not `assert_eq!`: a diff of 169 kB of JSON is no message.
        assert!(
            here == there,
            "{name} differs from the description's model (display-hive.md § 0.7)"
        );
    }
}
