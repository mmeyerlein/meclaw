//! Welle D -- the scene's seconds and its chime, driven by a real browser.
//!
//! The file beside this one asserts the identifiers. This one proves the
//! thing runs: `workshop/tools/display-scene-browser.mjs` builds a page out of
//! the hook's own source, calls `mounted` the way LiveView would, makes a
//! window arrive urgent and reads the counters back.
//!
//! Nothing is installed for it. The driver speaks CDP over the WebSocket Node
//! has had since 22, and the browser is the one Playwright already put in this
//! host's cache. Without either, the driver says `SKIP` and this test passes --
//! the same tool guard every other one in this tree uses (R2b).

use std::process::Command;

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const COMPOSE: &str = "templates/display/compose/compose.py";
const DRIVER: &str = "workshop/tools/display-scene-browser.mjs";

fn library_ships() -> bool {
    repo("templates/display/template.json").is_file()
}

/// The hook's source, as the browser gets it, written to a temp file for the
/// driver: asking `compose.py` for it is the only way to be sure the bytes
/// under test are the bytes that ship.
fn scene_js(to: &std::path::Path) -> Option<()> {
    let out = Command::new("python3")
        .arg("-c")
        .arg(
            "import importlib.util, sys\n\
             spec = importlib.util.spec_from_file_location('compose', sys.argv[1])\n\
             m = importlib.util.module_from_spec(spec)\n\
             spec.loader.exec_module(m)\n\
             open(sys.argv[2], 'w').write(m.SCENE_CLIENT_JS)",
        )
        .arg(repo(COMPOSE))
        .arg(to)
        .output()
        .ok()?;
    assert!(
        out.status.success(),
        "compose.py did not answer:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Some(())
}

fn counter(line: &str, key: &str) -> u64 {
    line.split_whitespace()
        .find_map(|part| part.strip_prefix(key))
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| panic!("{key} is not in {line:?}"))
}

#[test]
fn a_browser_runs_the_seconds_and_hears_the_timer() {
    if !library_ships() || !repo(DRIVER).is_file() {
        return;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    let js = td.path().join("scene.js");
    if scene_js(&js).is_none() {
        return;
    }
    let out = match Command::new("node").arg(repo(DRIVER)).arg(&js).output() {
        Ok(out) => out,
        Err(_) => return,
    };
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if out.status.code() == Some(3) || stderr.contains("SKIP") {
        println!("{stderr}");
        return;
    }
    assert!(
        out.status.success(),
        "the driver failed:\n{stdout}\n{stderr}"
    );
    let line = stdout
        .lines()
        .find(|l| l.starts_with("SCENE "))
        .unwrap_or_else(|| panic!("the driver printed no counters:\n{stdout}\n{stderr}"));
    println!("{line}");
    assert!(
        counter(line, "ticks=") >= 2,
        "the interval ran in a real engine: {line}"
    );
    assert!(
        counter(line, "chimes=") >= 1,
        "and an arriving urgent window was heard: {line}"
    );
    assert!(
        line.contains("shown=") && !line.contains("shown=--:--"),
        "and the remainder was written where the template said: {line}"
    );
}
