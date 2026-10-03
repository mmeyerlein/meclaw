//! GH #959 -- the presenter's rules, as tables over the pure functions of its `stage` cell.
//!
//! The driver is Python and loads `templates/presenter/stage/stage.py`; this test runs the
//! driver so the tables stand in the nextest filter and every diff under
//! `templates/presenter` pulls them (`docs/development-rules.md` § 10). The tables: the
//! manifest check, the questions of the one call (all topics plus `none`, 1 + 2N, the
//! ceiling, the state is the turn's text alone), the threshold, the binding, the audience
//! gate (`covers`, fail-closed), whole turns through the cell's entry with a held clock
//! (sure, unsure, `none`, error, unconfigured, timeout then late, invalid lead -> standard,
//! no data -> withdraw, a killed child), and every emission against `contract.emits`. The
//! driver also refuses a `config.json` whose `script_inline` drifted from `stage.py`.
//!
//! SKIPs where the template does not travel (R2b, the guard of every tool-bound test).

use std::process::Command;

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const DRIVER: &str = "templates/presenter/stage/check_stage.py";

#[test]
fn the_stage_rules_hold() {
    if !repo(DRIVER).exists() {
        println!("SKIP the_stage_rules_hold: templates/presenter does not travel");
        return;
    }
    let out = Command::new("python3")
        .arg(repo(DRIVER))
        .output()
        .expect("python3 runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "the stage tables are not green:\n{stdout}\n{stderr}"
    );
    let line = stdout
        .lines()
        .find_map(|l| l.strip_prefix("STAGE "))
        .unwrap_or_else(|| panic!("the driver printed no STAGE line:\n{stdout}"));
    let (ok, n) = line.split_once('/').expect("a counter");
    assert_eq!(ok, n, "STAGE {line}");
    // Every table ran: a table that silently vanished would leave a smaller, green count.
    for table in [
        "MANIFEST",
        "QUESTIONS",
        "THRESHOLD",
        "BINDING",
        "AUDIENCE",
        "TURNS",
        "CONTRACT",
    ] {
        assert!(
            stdout.lines().any(|l| l.starts_with(&format!("{table} "))),
            "the table {table} did not run:\n{stdout}"
        );
    }
}
