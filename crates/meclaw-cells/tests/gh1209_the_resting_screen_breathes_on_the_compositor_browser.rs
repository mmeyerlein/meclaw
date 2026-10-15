//! GH #1209 -- the resting screen breathes on the compositor.
//!
//! The screen blocked itself. Measured with the display lab's page (nine windows, a
//! front urgent, a modal, a dock of 13 tiles) on a lane: under a CPU quota of 80 %
//! WebKit drew 1-2 frames a second. The breaths of the urgent ring and of the
//! urgent notification animated `box-shadow`, a repaint every frame. Now they run
//! on `opacity` and `transform`; the resting picture is unchanged.
//!
//! The lock runs `workshop/tools/display-stall-browser.mjs` on the page the sheet
//! ships, in both engines, on the phone and on the monitor -- like every test stage
//! without filters (owner ruling 2026-10-12: the filters are about to change and
//! prove nothing here; the switch and its own lock are
//! `gh1209_no_stage_draws_a_filter_browser`). The verdict is the driver's
//! construction check, red on the sheet before the fix: every animation that runs,
//! endless or finite (a timer's colour ran for the timer's whole length, OR-T11-21) --
//! at rest, where the breaths of the fix are asked for by name, and in every state the
//! driver walks (open dock, status line, running step, urgent notification, a running
//! timer, the mark speaking, reduced motion) -- animates `transform`/`opacity` only,
//! with named exceptions in the driver. Every `@keyframes` of the sheet has run in one
//! of the states or is named there as not walked, and then its keyframes are the
//! verdict (fail-closed, whatever form the rules use them in); beside it, that
//! the page measured drew its windows and is the filter-free one. And the hang mark
//! (30 s): a main thread that never answers is `HANG:`, a slow one is not judged.
//! When the first frame was out, the longest gap and the frame rate are numbers in
//! the report and never an assertion (dev-rules section 11).
//!
//! **Why no CPU quota** -- the check does not depend on the CPU. The quota runs are
//! the report's measurement (TV-F7), not this lock.
//!
//! **Why SKIP and not RED** -- the tool guard of every browser proof in this tree
//! (R2b): without the templates, `node`, `playwright`, the bundle or the
//! laboratory, the test says `SKIP`; under
//! `MECLAW_REQUIRE_BROWSERS=1` (the gate's browser station) a skip is red.

use std::path::Path;
use std::process::Command;

use meclaw_core::serde_json::Value;

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const COMPOSE: &str = "templates/display/compose/compose.py";
const DRIVER: &str = "workshop/tools/display-stall-browser.mjs";
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

/// Sheet, scene and the mark's script as `compose.py` ships them, and the states the
/// driver walks (a status line, a running step, an urgent notification, a timer)
/// rendered from compose.py's own templates -- the animations that live outside the
/// lab's stage (§ M12f, M4 a; the timer since § M12h). The timer's epoch is a constant
/// far ahead: its animations stand in effect whatever the clock says.
fn page_parts(dir: &Path) -> Option<()> {
    let out = Command::new("python3")
        .arg("-c")
        .arg(
            "import importlib.util, re, sys\n\
             spec = importlib.util.spec_from_file_location('compose', sys.argv[1])\n\
             m = importlib.util.module_from_spec(spec)\n\
             spec.loader.exec_module(m)\n\
             d = sys.argv[2]\n\
             open(d + '/sheet.css', 'w').write(m.LAYOUT_RULES + m.KIT_CSS)\n\
             open(d + '/scene.js', 'w').write(m.SCENE_CLIENT_JS)\n\
             open(d + '/os.js', 'w').write(m.OS_CLIENT_JS)\n\
             def fill(t, **v):\n\
             \x20   t = re.sub(r'\\{\\{#if (\\w+)\\}\\}(.*?)\\{\\{/if\\}\\}', lambda x: x.group(2) if v.get(x.group(1)) else '', t)\n\
             \x20   return re.sub(r'\\{\\{(\\w+)\\}\\}', lambda x: str(v.get(x.group(1), '')), t)\n\
             step = fill(m.STEP_TEMPLATE, state='running', label='a step')\n\
             end = 10 ** 13\n\
             open(d + '/states.html', 'w').write(\n\
             \x20   fill(m.STATUS_TEMPLATE, kind='listening', text='listening')\n\
             \x20   + fill(m.STEPS_TEMPLATE, title='steps', children=step)\n\
             \x20   + fill(m.NOTIFICATION_TEMPLATE, level='urgent', title='a notice', children='')\n\
             \x20   + fill(m.TIMER_TEMPLATE, end_at=end, total_ms=7200000, now=end - 3600000, remaining='60:00', label='a timer'))",
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

fn the_resting_screen_answers(engine: &str, profile: &str) {
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
    println!("{engine}/{profile}: {}", report["measured"]);
    // Early warning, never a verdict: the sheet's `@keyframes` on more than
    // transform/opacity (the verdict on them is the driver's `compositor`, § M12h).
    if report["early_warning"]["off"]
        .as_array()
        .is_some_and(|off| !off.is_empty())
    {
        println!("WARN {engine}/{profile}: {}", report["early_warning"]);
    }
    assert!(
        report["ok"] == true,
        "{engine}/{profile}: the resting screen does not breathe on the compositor, draws \
         a filter, or hung\n{report:#}\n{stderr}"
    );
}

#[test]
fn the_resting_screen_answers_in_webkit_on_the_phone() {
    the_resting_screen_answers("webkit", "phone");
}

#[test]
fn the_resting_screen_answers_in_webkit_on_the_monitor() {
    the_resting_screen_answers("webkit", "monitor");
}

#[test]
fn the_resting_screen_answers_in_chromium_on_the_phone() {
    the_resting_screen_answers("chromium", "phone");
}

#[test]
fn the_resting_screen_answers_in_chromium_on_the_monitor() {
    the_resting_screen_answers("chromium", "monitor");
}
