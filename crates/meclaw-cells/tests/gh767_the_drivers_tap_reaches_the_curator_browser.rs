//! The tap of the viewer driver reaches the curator (GH #767, wave G, strand g10).
//!
//! `gh767_the_page_canvas_draws_and_sends_browser.rs` beside this one drives the LOCAL
//! mode: a page built out of `compose.py`'s parts and a FAKE socket. That measures the
//! hook. What it cannot measure is the only thing the `tap` MODE of the same driver
//! exists for -- that a gesture on a tile travels a REAL LiveView socket, becomes a pass
//! and comes back as a level. There is no HTTP tap to fall back on: the `compose` node
//! sits behind the hive boundary and a `POST` at it is dead-lettered (OR-G38), so this
//! driver is the only road a tap has, and a road nobody locks is a road that silently
//! stops leading anywhere.
//!
//! It stopped leading. Measured on 20.09.2026 against the disposable colony of this wave
//! (`127.0.0.1:7964`, `meclaw 0.39.0` out of master `0e71bc5c9`): eight runs of
//! `--mode tap`, `TAP mode=tap tapped=1` every single time, not one `tap` in
//! `/colony/messages`, and the window's `data-level` unmoved at `0` before and after --
//! while `workshop/tools/display-lab/tap.mjs` moved the same window with the same kind of
//! synthetic click in the same second. The tile is in the SERVER-RENDERED markup and is
//! therefore there long before LiveView has joined; a `phx-click` dispatched in that
//! window reaches nobody, because the binding lives on the socket and not in the
//! attribute. `window.__displayScene` -- the scene hook, which LiveView only mounts once
//! the view is joined -- is the readiness the driver has to wait for, and it is the same
//! one `workshop/tools/display-lab/tap.mjs:126` has always waited for.
//!
//! That is what this lock holds: one window open, one tap through the driver, level 0 on
//! the window the pass drew. Marker G6 of the proof line reads exactly this over
//! three exits, and it was red through five runs for this reason and not because of the
//! screen (the same gesture from a browser that waits puts the window away every time).
//!
//! **Why SKIP and not RED.** The tool guard every browser proof in this tree uses (R2b):
//! no `playwright`, no browser bundle, no `node` -- the driver leaves with 3 and this
//! test passes. A host that cannot measure is not a finding about the screen.

#[path = "support/display_colony.rs"]
mod display_colony;

use std::process::Command;

use display_colony::{Boot, PROBE, attr, boot, have_python, library_ships, repo};
use meclaw_core::serde_json::json;

const DRIVER: &str = "workshop/tools/display-page-browser.mjs";

/// One field of the driver's line.
fn field(line: &str, key: &str) -> String {
    line.split_whitespace()
        .find_map(|part| part.strip_prefix(key))
        .unwrap_or_else(|| panic!("{key} is not in {line:?}"))
        .to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_drivers_tap_puts_the_window_away() {
    if !library_ships() || !have_python() {
        println!("SKIP no template library or no python3 on this host");
        return;
    }
    if !repo(DRIVER).is_file() {
        println!("SKIP the viewer driver does not ship in this tree");
        return;
    }

    // Linger and fade far longer than the run: the window has to still be standing when
    // the browser gets to it, and a lock that waits for a fade is a nap (the shape
    // `710_the_colony_holds_in_both_engines_browser.rs` uses for the same reason).
    let colony = boot(Boot {
        linger_ms: 900_000,
        fade_ms: 900_000,
        ..Boot::default()
    })
    .await;

    let oid = colony.oid(PROBE, "tapped");
    colony
        .put(
            PROBE,
            "tapped",
            json!({"title": "the window a finger puts away", "context": "conversation",
                   "relevance": "0.9"}),
        )
        .await;
    colony
        .wait_tree("the window stands open before the tap", |t| {
            attr(t, &oid, "level") == json!("1")
        })
        .await;

    let td = tempfile::TempDir::new().expect("tempdir");
    // The dock's `data-for` is the window's `pane_id`, which the support mints as
    // `pane-<view_id>` (`display_colony.rs`, `write_view_ttl`). `monitor` is the exit
    // whose profile carries `pointer`, so it is the one whose tiles are bound at all
    // (§ 6.4).
    let out = Command::new("node")
        .arg(repo(DRIVER))
        .arg(colony.url("monitor"))
        .arg(td.path())
        .args(["--mode", "tap", "--profile", "monitor", "--tap"])
        .arg(r#".display-tile[data-for="pane-tapped"]"#)
        .output();
    let Ok(out) = out else {
        println!("SKIP no node on this host");
        colony.shutdown().await;
        return;
    };
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if out.status.code() == Some(3) || stderr.contains("SKIP") {
        println!("{stderr}");
        colony.shutdown().await;
        return;
    }
    let line = stdout
        .lines()
        .find(|l| l.starts_with("TAP mode=tap"))
        .unwrap_or_else(|| panic!("the driver printed no line:\n{stdout}\n{stderr}"))
        .to_string();
    println!("{line}");

    assert_eq!(
        field(&line, "tapped="),
        "1",
        "a tile for the window stood on the monitor: {line}"
    );
    // The screen's own answer FIRST, and read where it lands: the level the pass drew at
    // `web` (GH #809 -- the curator's state is memory, the patches are the screen). This
    // is the half that was red: `tapped=1` above and a window that never moved.
    colony
        .wait_tree("the tap of the driver puts the window away (§ 5.2)", |t| {
            attr(t, &oid, "level") == json!("0")
        })
        .await;

    // And the driver says WHY it can promise that. `ready=0` is a driver that dispatched
    // into a page whose LiveView had not joined -- the events land in the DOM, the screen
    // never hears them, and `tapped=1` says nothing about it.
    assert_eq!(
        field(&line, "ready="),
        "1",
        "the driver waited for the screen's hook before it tapped: {line}"
    );

    colony.shutdown().await;
}
