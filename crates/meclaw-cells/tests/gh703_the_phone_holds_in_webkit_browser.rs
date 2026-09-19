//! Welle F -- the sheet in the engine an iPhone actually is (R-23-10).
//!
//! Chrome on iOS is WebKit, and so is every browser there. Everything this
//! wave built for the phone hangs on features whose behaviour differs from
//! Chromium's: `100dvh` against a moving address bar, `env(safe-area-inset-*)`
//! behind `viewport-fit=cover`, a `backdrop-filter` layer that WebKit painted
//! through an ancestor's opacity, `:has()` for the plane blur, and an
//! `AudioContext` that runs at 44 100 rather than 48 000.
//!
//! This test drives the SHEET half, out of a page the driver builds itself:
//! it needs no colony and runs in the gate. The colony half -- the toggle
//! that must switch exactly once, the tile tap, the hold -- is F4's, against
//! the throwaway colony, over loopback so the microphone exists at all.
//!
//! Nothing is installed by this test. The driver imports `playwright`, which
//! `workshop/tools/package.json` pins, the checked-in `package-lock.json`
//! fixes and `npm ci` puts in place (OR-F12);
//! without the module, without the browser bundle, or without the laboratory
//! in `$MECLAW_WKDEPS` it says `SKIP` and exits 3, and this test passes --
//! the same tool guard every other one in this tree uses (R2b).

use std::process::Command;

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const COMPOSE: &str = "templates/display/compose/compose.py";
const DRIVER: &str = "workshop/tools/display-webkit-browser.mjs";

fn library_ships() -> bool {
    repo("templates/display/template.json").is_file()
}

/// Sheet and scripts as the browser gets them, written beside each other for
/// the driver: asking `compose.py` is the only way to be sure the bytes under
/// test are the bytes that ship.
fn page_parts(dir: &std::path::Path) -> Option<()> {
    let out = Command::new("python3")
        .arg("-c")
        .arg(
            "import importlib.util, sys\n\
             spec = importlib.util.spec_from_file_location('compose', sys.argv[1])\n\
             m = importlib.util.module_from_spec(spec)\n\
             spec.loader.exec_module(m)\n\
             d = sys.argv[2]\n\
             open(d + '/sheet.css', 'w').write(m.LAYOUT_RULES + m.KIT_CSS)\n\
             open(d + '/scene.js', 'w').write(m.SCENE_CLIENT_JS)",
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

/// One `key=value` out of the driver's line.
fn field<'a>(line: &'a str, key: &str) -> &'a str {
    line.split_whitespace()
        .find_map(|part| part.strip_prefix(key))
        .unwrap_or_else(|| panic!("{key} is not in {line:?}"))
}

#[test]
fn the_sheet_behaves_in_webkit() {
    if !library_ships() || !repo(DRIVER).is_file() {
        return;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    if page_parts(td.path()).is_none() {
        return;
    }
    let out = match Command::new("node")
        .arg(repo(DRIVER))
        .arg(td.path())
        .arg(td.path().join("shots"))
        .output()
    {
        Ok(out) => out,
        Err(_) => return,
    };
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    // The driver's own SKIP, and only that: it says it on a line of its own
    // and leaves with 3. A plain `contains("SKIP")` would also match the line
    // Playwright prints whenever the laboratory is sourced -- it names
    // `PLAYWRIGHT_SKIP_VALIDATE_HOST_REQUIREMENTS` -- and a guard that swallows
    // every run inside the laboratory is a proof that cannot fail.
    if out.status.code() == Some(3) || stderr.lines().any(|l| l.starts_with("SKIP ")) {
        println!("{stderr}");
        return;
    }
    assert!(
        out.status.success(),
        "the driver failed:\n{stdout}\n{stderr}"
    );
    let line = stdout
        .lines()
        .find(|l| l.starts_with("WEBKIT "))
        .unwrap_or_else(|| panic!("the driver printed no line:\n{stdout}\n{stderr}"));
    println!("{line}");
    assert_eq!(
        field(line, "has="),
        "true",
        "`:has()` carries the plane blur: {line}"
    );
    assert_eq!(
        field(line, "dvh="),
        "true",
        "`100dvh` is the visible height, not the one without the address bar: {line}"
    );
    assert_eq!(
        field(line, "display="),
        "none",
        "a hidden dock is a box that was never made, in the engine that painted through opacity: {line}"
    );
    assert_eq!(
        field(line, "open_display="),
        "flex",
        "and `data-dock-open` on <html> brings it back: {line}"
    );
    // Playwright emulates no notch, so the inset is 0 here and the number is
    // reported rather than asserted: the safe area is accepted on the device
    // itself (R-23-10). What IS asserted is that the sheet asks.
    println!(
        "safe area as measured in the laboratory: {}",
        field(line, "safe=")
    );
}
