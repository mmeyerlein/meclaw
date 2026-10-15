//! GH #1209 -- no test stage draws a filter, and the product does not know it.
//!
//! Owner ruling (2026-10-12): in the tests the special filters are off --
//! `backdrop-filter`, `filter: blur()` and the like. They prove nothing a test asks
//! about, they are about to change, and a test has to be fast. The test driver
//! switches them off for every browser stage; the product carries no test branch.
//!
//! The switch is `workshop/tools/lib/no-filters.mjs`: one constructed style sheet,
//! adopted by every document a stage opens. Playwright stages get it through the
//! one door they all open the engine by (`openEngine` in
//! `display-browser-lab.mjs`); the drivers that speak CDP by hand call
//! `noFiltersCdp` on the session they attach.
//!
//! What this file locks:
//!
//! - **every stage** -- the lab's page (nine windows, a modal, a front urgent, the
//!   dock of glass tiles: the screen with the most filters on it), in both engines
//!   and on every screen class, opened the way a stage opens it, has no element and
//!   no pseudo element the probe reads whose computed `filter` or `backdrop-filter`
//!   is not `none`; in Chromium the same for a page switched the way a CDP driver
//!   switches it. The switch is a cascade layer of `!important` rules, so no
//!   unlayered rule of the sheet beats it, whatever its specificity; a filter on a
//!   pseudo element the probe cannot read (`::-webkit-*`, `::part()`, ...) is red in
//!   the sheet check `forms`, as is a sheet that cannot be read (§ M12h). The positive control rides along: the same page in the engine as
//!   it ships DOES draw filters (the probe sees them, and the sheet keeps its glass).
//!   The red line is the stage without the switch (`--control`), recorded in the
//!   strand's report;
//! - **every driver goes through the switch** -- no driver launches an engine but
//!   the lab's door, and every CDP attach is followed by the switch;
//! - **the product carries no switch** -- locked in its own binary,
//!   `gh1209_the_product_carries_no_test_switch.rs`: it reads every crate's `src/`,
//!   and in a browser unit that read moved the browser station into every Rust diff.
//!
//! **Why SKIP and not RED** -- the tool guard of every browser proof in this tree
//! (R2b): without the templates, `node`, `playwright`, the bundle or the
//! laboratory, the test says `SKIP`; under `MECLAW_REQUIRE_BROWSERS=1` (the gate's
//! browser station) a skip is red.

use std::path::{Path, PathBuf};
use std::process::Command;

use meclaw_core::serde_json::Value;

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const COMPOSE: &str = "templates/display/compose/compose.py";
const DRIVER: &str = "workshop/tools/display-filters-browser.mjs";
const TOOLS: &str = "workshop/tools";
const LAB: &str = "display-browser-lab.mjs";
const SWITCH: &str = "lib/no-filters.mjs";
/// The laboratory WebKit runs in on this host, sourced for every run except a Chromium one.
const WKENV: &str = "workshop/tools/wkenv.sh";
/// `sh -c` with `$1` the laboratory, the rest the driver and its arguments.
const WITH_LAB: &str =
    "case \" $* \" in *\" --engine chromium \"*) ;; *) . \"$1\" ;; esac; shift; exec node \"$@\"";
fn library_ships() -> bool {
    repo("templates/display/template.json").is_file()
}

fn skip(why: &str) {
    println!("SKIP {why}");
    assert_ne!(
        std::env::var("MECLAW_REQUIRE_BROWSERS").as_deref(),
        Ok("1"),
        "SKIP under MECLAW_REQUIRE_BROWSERS=1 is red: {why}"
    );
}

/// Sheet, scene and the mark's script as `compose.py` ships them.
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

fn the_stage_draws_no_filter(engine: &str, profile: &str) {
    if !library_ships() {
        return skip("the template library does not ship in this tree");
    }
    if !repo(DRIVER).is_file() || !repo(WKENV).is_file() {
        return skip("the browser driver does not ship in this tree");
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    if page_parts(td.path()).is_none() {
        return skip("no python3 on this host");
    }
    let out = match Command::new("sh")
        .arg("-c")
        .arg(WITH_LAB)
        .arg("sh")
        .arg(repo(WKENV))
        .arg(repo(DRIVER))
        .arg(td.path())
        .arg(td.path().join("out"))
        .arg("--engine")
        .arg(engine)
        .arg("--profile")
        .arg(profile)
        .output()
    {
        Ok(out) => out,
        Err(e) => {
            return skip(&format!(
                "neither node nor a shell for it on this host: {e}"
            ));
        }
    };
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if out.status.code() == Some(3) || stderr.lines().any(|l| l.starts_with("SKIP ")) {
        // the switch is read in this branch, not only in `skip`: the lock
        // (`test_browser_required`) reads each exit-3 branch up to its `return`
        let why = format!("{engine}: {}", stderr.trim());
        assert_ne!(
            std::env::var("MECLAW_REQUIRE_BROWSERS").as_deref(),
            Ok("1"),
            "SKIP under MECLAW_REQUIRE_BROWSERS=1 is red: {why}"
        );
        return skip(&why);
    }
    let line = stdout
        .lines()
        .find(|l| l.starts_with('{'))
        .unwrap_or_else(|| panic!("the driver printed no report:\n{stdout}\n{stderr}"));
    let report: Value = meclaw_core::serde_json::from_str(line).expect("the report is JSON");
    println!("{engine}/{profile}: {}", report["checks"]);
    // Early warning, never a verdict (§ M12f, M4 b): a filter form the probe measures but
    // no style sheet can switch (`!important` in a page layer or inline).
    if report["early_warning"]["forms"].as_u64().unwrap_or(0) > 0 {
        println!("WARN {engine}/{profile}: {}", report["early_warning"]);
    }
    assert!(
        report["ok"] == true,
        "{engine}/{profile}: a test stage draws a filter, or the control draws none\n\
         {report:#}\n{stderr}"
    );
}

#[test]
fn no_stage_draws_a_filter_in_webkit_on_the_phone() {
    the_stage_draws_no_filter("webkit", "phone");
}

#[test]
fn no_stage_draws_a_filter_in_webkit_on_the_monitor() {
    the_stage_draws_no_filter("webkit", "monitor");
}

#[test]
fn no_stage_draws_a_filter_in_webkit_on_the_tv() {
    the_stage_draws_no_filter("webkit", "tv");
}

#[test]
fn no_stage_draws_a_filter_in_chromium_on_the_phone() {
    the_stage_draws_no_filter("chromium", "phone");
}

#[test]
fn no_stage_draws_a_filter_in_chromium_on_the_monitor() {
    the_stage_draws_no_filter("chromium", "monitor");
}

#[test]
fn no_stage_draws_a_filter_in_chromium_on_the_tv() {
    the_stage_draws_no_filter("chromium", "tv");
}

/// Every file under `dir` whose name ends in one of `exts`, `node_modules` aside.
fn files(dir: &Path, exts: &[&str], out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            if name != "node_modules" && !name.starts_with('.') {
                files(&path, exts, out);
            }
        } else if exts.iter().any(|e| name.ends_with(e)) && !name.starts_with(".env") {
            out.push(path);
        }
    }
}

#[test]
fn every_browser_driver_goes_through_the_switch() {
    let tools = repo(TOOLS);
    if !tools.join(LAB).is_file() {
        return skip("the browser drivers do not ship in this tree");
    }
    let mut all = Vec::new();
    files(&tools, &[".mjs", ".js"], &mut all);
    let lab = std::fs::read_to_string(tools.join(LAB)).expect("the lab reads");
    assert!(
        lab.contains("if (got.browser) withoutFilters(got.browser);"),
        "{LAB}: `openEngine` hands out every browser with the switch applied"
    );
    let mut cdp_drivers = 0;
    for path in &all {
        let rel = path.strip_prefix(&tools).expect("under tools");
        let rel = rel.to_string_lossy();
        if rel == SWITCH {
            continue;
        }
        let text = std::fs::read_to_string(path).expect("a driver reads");
        if rel != LAB {
            assert!(
                !text.contains(".launch(") && !text.contains("launchPersistentContext"),
                "{rel}: launches an engine past the lab's door (`openEngine`), so past the switch"
            );
        }
        if text.contains("launchEngine") {
            assert!(
                rel == LAB || rel == "display-filters-browser.mjs",
                "{rel}: opens the engine as it ships; only the switch's control may"
            );
        }
        let attaches = text.matches("\"Target.attachToTarget\"").count();
        let switched = text
            .matches("await noFiltersCdp((m, p) => cdp.send(m, p, sessionId));")
            .count();
        if text.contains("--remote-debugging-port") || attaches > 0 {
            cdp_drivers += 1;
            assert!(
                attaches > 0 && switched == attaches,
                "{rel}: a CDP driver switches the filters off on every page it attaches \
                 ({attaches} attaches, {switched} switched)"
            );
        }
    }
    assert!(
        cdp_drivers >= 7,
        "the CDP drivers were found ({cdp_drivers}): the lock reads the tree it guards"
    );
}
