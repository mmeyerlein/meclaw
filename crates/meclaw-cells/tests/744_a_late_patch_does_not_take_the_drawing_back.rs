//! GH #744 — a patch from a pass that started before the tap does not take the
//! optimistic drawing back (display-hive.md § 5.7, Decision 18.09.).
//!
//! § 5.7 promises that the pass never disagrees with what the client drew. It
//! keeps that promise about the CONTENT and said nothing about the ORDER, and
//! the order is what a person feels. Every event costs a full read pass, so a
//! finger that taps while a pass is still in the air is answered first by THAT
//! pass — which renders the state from before the tap and writes it over the
//! drawing (`crates/meclaw-cells/src/web/cell.rs`: what a bundle carries is
//! written over the DOM).
//!
//! Measured on the candidate colony, the owner's own session
//! (`plans/welle-h3-2026-09-18/messungen/B-klickserie.md` § 3, 16:38:16–16:38:20):
//! five taps on the chat tile in four seconds, every one of them processed by the
//! curator, and the person saw a window that flashed open and closed again each
//! time. It looked exactly like a tile that does nothing, and only a reload
//! ended it.
//!
//! What is locked here: the client keeps its drawing for a tapped window until a
//! patch arrives whose state was computed after the tap, and it reads that off
//! the curator's own stamp `data-acted` — the last moment a touch or a put-away
//! moved this window (`max(since, dismissed_at)`), which is the browser-visible
//! version of what § 5.2 and § 4.9 write. No clock of the browser's own enters
//! the comparison: the stamp is the SERVER's, read out of the DOM at the moment
//! the finger lands, so a skewed device clock cannot release the hold early.
//!
//! The source half of this claim is `708_the_client_draws_the_tap_before_the_pass.rs`;
//! it is needles in a string and a one-line mutant keeps every one of them
//! standing. This half runs the hook in a real engine and delivers two late
//! frames.
//!
//! Skips when the templates, the driver, node or a browser are missing (R2b) —
//! this tree installs nothing for a test.

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

/// The hook's source, as the browser gets it. Asking `compose.py` for it is the
/// only way to be sure the bytes under test are the bytes that ship.
fn scene_js(to: &std::path::Path) -> Option<()> {
    let out = std::process::Command::new("python3")
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

/// One `key=value` of the driver's line.
fn field<'a>(line: &'a str, key: &str) -> &'a str {
    line.split_whitespace()
        .find_map(|p| p.strip_prefix(key))
        .unwrap_or_else(|| panic!("{key} is not in {line:?}"))
}

/// The tapped canvas window and the modal it co-closes, in a real engine, with
/// two late frames delivered by hand.
///
/// The driver prints `tap=<before>/<drawn>/<held>/<reopened>/<released>`, each
/// reading the two `data-level` values in that order (timer, chat):
///
/// ```text
/// built              tap=32/00/00/02/12 restored=2
/// before the fix     tap=32/00/12/12/12 restored=0
/// before the co-close guard  tap=32/00/00/00/12 restored=2
/// ```
///
/// `held` and `reopened` are the two cases. `before` and `drawn` say only that the
/// page is the one this measures and that § 5.7's first half still works;
/// `released` says the hold ends, which is what keeps a drawing from outliving the
/// pass that answers it.
#[test]
fn a_patch_of_an_older_pass_leaves_the_drawing_standing() {
    if !library_ships() || !repo(DRIVER).is_file() {
        return;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    let js = td.path().join("scene.js");
    if scene_js(&js).is_none() {
        return;
    }
    let out = match std::process::Command::new("node")
        .arg(repo(DRIVER))
        .arg(&js)
        .output()
    {
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
        .unwrap_or_else(|| panic!("the driver printed no SCENE line:\n{stdout}\n{stderr}"));
    let tap = field(line, "tap=");
    let steps: Vec<&str> = tap.split('/').collect();
    assert_eq!(steps.len(), 5, "tap= is five readings: {line}");
    assert_eq!(
        steps[0], "32",
        "the page this measures starts with the canvas window on the urgent level \
         (the driver delivers one before this block) and the modal over it: {line}"
    );
    assert_eq!(
        steps[1], "00",
        "the client draws the tap itself -- the window is put away and the modal \
         co-closed in the same frame (§ 5.2, § 5.3): {line}"
    );
    assert_eq!(
        steps[2], "00",
        "a patch whose state was computed BEFORE the tap took the drawing back. \
         § 5.7 (Decision 18.09.): the client keeps its drawing until a patch \
         carries a state computed after the tap -- this is the flash the owner \
         read as `the tile does nothing` (B-klickserie § 3, e25 16:38:16): {line}"
    );
    assert_eq!(
        steps[3], "02",
        "a patch that moves the CO-CLOSED window's own stamp shut it again. § 5.7 \
         runs in both directions, and the state is one for all outputs (§ 3.1): a \
         chat opened by a hold (§ 5.4 -- it never passes through the tile handler) \
         or a tap on the phone reaches this browser only as a patch, and a list \
         drawn seconds ago may not close it. The tapped window is still held here, \
         because ITS stamp has not moved: {line}"
    );
    assert_eq!(
        steps[4], "12",
        "and a patch stamped AFTER the tap stands again, whatever it says -- a \
         hold that never ends is a client that stopped listening: {line}"
    );
    assert_eq!(
        field(line, "restored="),
        "2",
        "two patches were drawn over: a `held` that is right because nothing \
         patched at all proves nothing: {line}"
    );
}
