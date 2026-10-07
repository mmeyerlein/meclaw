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
//! about the device this screen is carried on. One `#[test]` per engine and part rather
//! than one test with all the runs: a red run then names the engine and the part in the
//! test name, and nextest runs them side by side instead of one after the other.
//!
//! **Why parts (GH #1048).** The sheet line ran as ONE driver run per engine, fifteen
//! proofs on one page: WebKit took 24.9 s alone in the station `browser:display`
//! (2026-10-02), 70-78 s in integration runs and 92.8 s in one strand gate (2026-10-06),
//! of a 240 s budget with the mark at 80 s. Two kinds of time make that up. About 19 s
//! is B-33's own clock and does not shrink under load: a 300 ms tap, two 4.5 s holds
//! against the 3 s threshold, a 3.5 s stall twice and the settles between them
//! (`display-layout-browser.mjs`, `B33`). The rest is WebKit painting a 393x852 page at
//! dpr 3 with Mesa in software (`wkenv.sh`), and THAT grows with the load -- the same
//! line went from 25 s to 93 s. A part is one driver run over a contiguous slice of the
//! line (`--checks`), with its own engine start and its own page, so the longest part is
//! B-33's clock plus one start, and the CPU half is spread over four processes instead
//! of one. The WebKit parts take turns (`.config/nextest.toml`, test group
//! `webkit`): side by side they starved each other. Every assertion is still made
//! per proof; nothing below judges across parts.
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
/// The laboratory WebKit runs in on this host, sourced for every run except a Chromium
/// one (the driver's default engine is WebKit). Sourced for Chromium too, a host without
/// the laboratory read its `SKIP` line as the Chromium run's own, and the lock proved
/// nothing there (XB-M review I-1: build02 skipped every Chromium run).
const WKENV: &str = "workshop/tools/wkenv.sh";
/// `sh -c` with `$1` the laboratory, the rest the driver and its arguments.
const WITH_LAB: &str =
    "case \" $* \" in *\" --engine chromium \"*) ;; *) . \"$1\" ;; esac; shift; exec node \"$@\"";

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

/// Drive one engine through `checks`, a part of the sheet line of befund 04 § E.3, or
/// `None` when this host cannot measure.
///
/// The driver is run through `sh` so that `wkenv.sh` is sourced first: Playwright's own
/// wrapper sets `LD_LIBRARY_PATH`, so the laboratory has to be in the environment of the
/// `node` process itself and cannot be handed over afterwards. Where there is no
/// laboratory, `wkenv.sh` writes its own `SKIP ` line and the guard below reads it.
fn drive(engine: &str, checks: &[&str], parts: &Path, out_dir: &Path) -> Option<Value> {
    if !repo(DRIVER).is_file() || !repo(WKENV).is_file() {
        println!("SKIP the browser driver does not ship in this tree");
        return None;
    }
    let run = Command::new("sh")
        .arg("-c")
        .arg(WITH_LAB)
        .arg("sh")
        .arg(repo(WKENV))
        .arg(repo(DRIVER))
        .arg(parts)
        .arg(out_dir)
        .arg("--engine")
        .arg(engine)
        .arg("--run")
        .arg("sheet")
        // The part: the driver runs exactly these proofs, in this order, on a page of
        // their own. The line itself stays the driver's (`RUNS.sheet`), and the lock at
        // the foot of this file holds the parts to it.
        .arg("--checks")
        .arg(checks.join(","))
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

/// One engine and one part, end to end: parts out of `compose.py`, the proofs of the
/// part, every check -- and exactly the checks that were asked for, so a driver that
/// ignored `--checks` and ran the whole line (or nothing) is red here.
fn the_sheet_holds_in(engine: &str, checks: &[&str]) {
    if !library_ships() {
        println!("SKIP the template library does not ship in this tree");
        return;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    if page_parts(td.path()).is_none() {
        println!("SKIP no python3 on this host");
        return;
    }
    let Some(report) = drive(engine, checks, td.path(), &td.path().join("shots")) else {
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
    let mut ran: Vec<&str> = report["checks"]
        .as_object()
        .map(|c| c.keys().map(String::as_str).collect())
        .unwrap_or_default();
    ran.sort_unstable();
    let mut asked = checks.to_vec();
    asked.sort_unstable();
    assert_eq!(
        ran, asked,
        "{engine}: the driver ran other proofs than the part"
    );
    assert_every_check_holds(engine, &report);
}

// ─────────────────────────────────────────────────────────────── the parts

/// The sheet line of the driver (`RUNS.sheet`), cut into contiguous slices: the order
/// inside a part is the order of the line, and every part starts on a fresh page -- the
/// page the first proof of the whole line used to get.
///
/// The page, the dock, the blur and the tiles stay ONE part: cut after B-18, B-21 ("the
/// seconds run down", a four-tick deadline) met a fresh WebKit page still busy with its
/// first software-rendered paint and failed 5 of 10 stress iterations on build01 under
/// load (2026-10-07, `the seconds do not run down in the browser (D-27)`); on the page
/// B-02..B-18 already settled -- the page it always had -- it holds.
const PAGE_AND_TILES: &[&str] = &[
    "B-02", "B-08", "B-09", "B-13", "B-16", "B-17", "B-18", "B-20", "B-21", "B-22", "B-27",
];
/// B-33 alone: about 19 s of fixed press and stall time, the longest part on its own.
const HOLD: &[&str] = &["B-33"];
/// B-34 alone: it builds a page for each of the three exits.
const CAPTION: &[&str] = &["B-34"];
const WINDOWS_AND_FADE: &[&str] = &["B-35", "B-36"];

/// One `#[test]` per engine and part (GH #1048, the WHY is at the head of this file).
macro_rules! cells {
    ($($name:ident => ($engine:expr, $checks:expr);)+) => {
        /// Every cell the macro made, in the order it made them.
        const CELLS: &[(&str, &[&str])] = &[$(($engine, $checks)),+];
        $(
            #[test]
            fn $name() {
                the_sheet_holds_in($engine, $checks);
            }
        )+
    };
}

cells! {
    the_sheet_holds_in_chromium_page_and_tiles => ("chromium", PAGE_AND_TILES);
    the_sheet_holds_in_chromium_hold => ("chromium", HOLD);
    the_sheet_holds_in_chromium_caption => ("chromium", CAPTION);
    the_sheet_holds_in_chromium_windows_and_fade => ("chromium", WINDOWS_AND_FADE);
    the_sheet_holds_in_webkit_page_and_tiles => ("webkit", PAGE_AND_TILES);
    the_sheet_holds_in_webkit_hold => ("webkit", HOLD);
    the_sheet_holds_in_webkit_caption => ("webkit", CAPTION);
    the_sheet_holds_in_webkit_windows_and_fade => ("webkit", WINDOWS_AND_FADE);
}

/// The proofs of the driver's sheet line, in its order, read off `RUNS.sheet` in the
/// driver's source -- the line is the driver's, and this file only cuts it.
fn the_sheet_line() -> Vec<String> {
    let src = std::fs::read_to_string(repo(DRIVER)).expect("the driver reads");
    let at = src
        .find("\n  sheet: [")
        .expect("the driver carries a sheet line (`RUNS.sheet`)");
    let rest = &src[at..];
    let body = &rest[rest.find('[').expect("[") + 1..rest.find(']').expect("]")];
    body.split('"')
        .filter(|t| t.starts_with("B-"))
        .map(str::to_string)
        .collect()
}

/// The split loses no proof: for each engine the parts, one after the other, are the
/// driver's sheet line exactly -- same proofs, same order, none twice. A proof added to
/// `RUNS.sheet` without a part is red here, not silently unmeasured.
#[test]
fn the_parts_are_the_sheet_line() {
    if !repo(DRIVER).is_file() {
        println!("SKIP the browser driver does not ship in this tree");
        return;
    }
    let line = the_sheet_line();
    assert!(!line.is_empty(), "the sheet line read empty off the driver");
    for engine in ["chromium", "webkit"] {
        let have: Vec<String> = CELLS
            .iter()
            .filter(|(e, _)| *e == engine)
            .flat_map(|(_, checks)| checks.iter().map(|c| c.to_string()))
            .collect();
        assert_eq!(
            have, line,
            "{engine}: the parts of this file are not the driver's sheet line; \
             add the proof to a part (or a part to the cells! list)"
        );
    }
}
