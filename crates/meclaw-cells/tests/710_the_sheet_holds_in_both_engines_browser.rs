//! The sheet half of the B-proofs, in BOTH engines (befund 04 § E.3, § 6.7).
//!
//! Ten of the twenty-nine "Not in the model" sentences of display-hive.md § 5-9 are
//! decided by the sheet and the two client hooks alone: the dock's default after a load
//! (B-02), the page that never scrolls and the window that scrolls inside (B-08, B-09),
//! the hover that is always guarded (B-13), the `--scale` of § 6.6 (B-16), the phone
//! sheet with `100dvh`, `viewport-fit` and a field no engine may zoom into (B-17), the
//! point on the mark (B-18), the blur that belongs to modal moments only (B-20), the dock
//! of equal tiles (B-21) and the opacity no tile falls below (B-22). None of them needs a
//! colony: the driver builds the page itself out of `sheet.css` and `scene.js` as
//! `compose.py` ships them, so this file runs in the gate as the station
//! `browser:display` with scope `sheet`.
//!
//! **Why both engines.** § 6.7 and R-23-10: every browser on an iPhone is WebKit, and
//! the things this display leans on are exactly the ones whose behaviour differs there --
//! `100dvh` against a moving address bar, `env(safe-area-inset-*)` behind
//! `viewport-fit=cover`, a `backdrop-filter` layer WebKit painted through an ancestor's
//! opacity, `:has()` for the plane blur. A proof measured in Chromium alone says nothing
//! about the device this screen is carried on. One `#[test]` per engine rather than one
//! test with two runs: a red run then names the engine in the test name, and nextest runs
//! the two side by side instead of one after the other.
//!
//! **Why SKIP and not RED.** Nothing here is installed by the test. `playwright` is the
//! one npm dependency of `workshop/tools/`, pinned at 1.63.0 and put in place by
//! `npm ci`; the WebKit bundle needs the laboratory `wkenv.sh` describes, which is an ops
//! step and not one a proof gets to take (OR-F12). Without the module, without the
//! bundle, without the laboratory or without `node`, the driver says `SKIP` on a line of
//! its own and leaves with 3, and this test passes -- the same tool guard every other
//! browser proof in this tree uses (R2b). A host that cannot measure is not a finding
//! about the sheet.

use std::path::Path;
use std::process::Command;

use meclaw_core::serde_json::Value;

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const COMPOSE: &str = "templates/display/compose/compose.py";
const DRIVER: &str = "workshop/tools/display-layout-browser.mjs";
/// The laboratory WebKit runs in on this host. Sourced for BOTH engines, so a run has
/// one shape: Chromium ignores every variable in it, and reading the file in one place
/// is better than two code paths of which only one is ever exercised (OR-H5.3).
const WKENV: &str = "workshop/tools/wkenv.sh";

/// Whether the template library travels in this tree (it does not in the published one).
fn library_ships() -> bool {
    repo("templates/display/template.json").is_file()
}

/// Sheet and scripts as the browser gets them, written beside each other for the driver:
/// asking `compose.py` is the only way to be sure the bytes under test are the bytes that
/// ship. `os.js` rides along, so the mark's own hook is mounted and B-02's press is a
/// real gesture rather than an attribute set by hand.
fn page_parts(dir: &Path) -> Option<()> {
    let out = Command::new("python3")
        .arg("-c")
        .arg(
            "import importlib.util, sys\n\
             spec = importlib.util.spec_from_file_location('compose', sys.argv[1])\n\
             m = importlib.util.module_from_spec(spec)\n\
             spec.loader.exec_module(m)\n\
             d = sys.argv[2]\n\
             open(d + '/sheet.css', 'w').write(m.LAYOUT_RULES + m.KIT_CSS)\n\
             open(d + '/scene.js', 'w').write(m.SCENE_CLIENT_JS)\n\
             open(d + '/os.js', 'w').write(m.OS_CLIENT_JS)",
        )
        .arg(repo(COMPOSE))
        .arg(dir)
        .output()
        .ok()?;
    assert!(
        out.status.success(),
        "compose.py did not answer:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Some(())
}

/// Drive one engine through the sheet line of befund 04 § E.3, or `None` when this host
/// cannot measure.
///
/// The driver is run through `sh` so that `wkenv.sh` is sourced first: Playwright's own
/// wrapper sets `LD_LIBRARY_PATH`, so the laboratory has to be in the environment of the
/// `node` process itself and cannot be handed over afterwards. Where there is no
/// laboratory, `wkenv.sh` writes its own `SKIP ` line and the guard below reads it.
fn drive(engine: &str, parts: &Path, out_dir: &Path) -> Option<Value> {
    if !repo(DRIVER).is_file() || !repo(WKENV).is_file() {
        println!("SKIP the browser driver does not ship in this tree");
        return None;
    }
    let run = Command::new("sh")
        .arg("-c")
        .arg(". \"$1\"; shift; exec node \"$@\"")
        .arg("sh")
        .arg(repo(WKENV))
        .arg(repo(DRIVER))
        .arg(parts)
        .arg(out_dir)
        .arg("--engine")
        .arg(engine)
        .arg("--run")
        .arg("sheet")
        // The phone: B-17 is a phone sentence, and the exit with the fewest pixels and
        // the most engine behind it is the one the other nine are hardest on.
        .arg("--profile")
        .arg("phone")
        .output();
    let out = match run {
        Ok(out) => out,
        Err(e) => {
            println!("SKIP neither node nor a shell for it on this host: {e}");
            return None;
        }
    };
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    // The driver's own SKIP, and only that: it says it on a line of its own and leaves
    // with 3. A plain `contains("SKIP")` would also match the line Playwright prints
    // whenever the laboratory is sourced -- it names
    // `PLAYWRIGHT_SKIP_VALIDATE_HOST_REQUIREMENTS` -- and a guard that swallows every run
    // inside the laboratory is a proof that cannot fail.
    if out.status.code() == Some(3) || stderr.lines().any(|l| l.starts_with("SKIP ")) {
        println!("SKIP {engine}: {}", stderr.trim());
        return None;
    }
    // Exit 1 is a failing proof and NOT a reason to leave early: the report is on stdout
    // either way, and the names in it are what makes a red fixable.
    let line = stdout
        .lines()
        .find(|l| l.starts_with('{'))
        .unwrap_or_else(|| {
            panic!(
                "the driver printed no report ({:?}):\n{stdout}\n{stderr}",
                out.status.code()
            )
        });
    Some(meclaw_core::serde_json::from_str(line).expect("the driver's report is JSON"))
}

/// Every check of one report, or a panic naming all of them.
///
/// A check that says `skipped: true` is allowed and printed by name: a proof this mode
/// cannot see at all is honest about it, and half a run is worth having. A check that
/// says `ok: false` takes the WHOLE `checks` block into the panic -- a B-number without
/// the values beside it cannot be fixed, and the values are what tell a sheet defect from
/// a driver that measures the wrong thing.
fn assert_every_check_holds(engine: &str, report: &Value) {
    let checks = report["checks"]
        .as_object()
        .unwrap_or_else(|| panic!("{engine}: the report carries no checks: {report}"));
    assert!(
        !checks.is_empty(),
        "{engine}: the sheet line measured nothing at all: {report}"
    );
    let mut failed: Vec<&str> = Vec::new();
    for (name, check) in checks {
        if check["skipped"] == Value::Bool(true) {
            println!(
                "{engine} {name}: skipped -- {}",
                check["why"].as_str().unwrap_or("")
            );
        }
        if check["ok"] != Value::Bool(true) {
            failed.push(name);
        }
    }
    assert!(
        failed.is_empty(),
        "{engine} ({}): {} of the sheet's B-proofs do not hold -- {}\n{}",
        report["viewport"].as_str().unwrap_or("?"),
        failed.len(),
        failed.join(", "),
        meclaw_core::serde_json::to_string_pretty(&report["checks"]).expect("json")
    );
    println!(
        "{engine} {} {}: {} proofs, all of them good",
        report["profile"].as_str().unwrap_or("?"),
        report["viewport"].as_str().unwrap_or("?"),
        checks.len()
    );
}

/// One engine, end to end: parts out of `compose.py`, the sheet line, every check.
fn the_sheet_holds_in(engine: &str) {
    if !library_ships() {
        println!("SKIP the template library does not ship in this tree");
        return;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    if page_parts(td.path()).is_none() {
        println!("SKIP no python3 on this host");
        return;
    }
    let Some(report) = drive(engine, td.path(), &td.path().join("shots")) else {
        return;
    };
    assert_eq!(
        report["mode"], "sheet",
        "the driver built the page itself; a colony here would measure something else"
    );
    assert_eq!(
        report["engine"], engine,
        "the run is the engine that was asked for"
    );
    assert_every_check_holds(engine, &report);
}

#[test]
fn the_sheet_holds_in_chromium() {
    the_sheet_holds_in("chromium");
}

#[test]
fn the_sheet_holds_in_webkit() {
    the_sheet_holds_in("webkit");
}
