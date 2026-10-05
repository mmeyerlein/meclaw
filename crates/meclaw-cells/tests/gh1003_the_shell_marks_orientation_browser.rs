//! GH #1003 T8: a turn of the phone is CSS only.
//!
//! The content of every screen is the same (R-H4-1); what a screen class
//! changes is layout, and a phone turned from portrait to landscape changes it
//! through `data-orientation` on `<html>`, which the shell's `boot.js` keeps in
//! step with the viewport (debounced 150 ms). No frame and no event may reach the
//! server for it — a server round trip per turn would make content depend on the
//! screen. Headless Chromium opens the page as an upright phone, turns it, and
//! `workshop/tools/display-orientation-browser.mjs` reports the attribute before
//! and after, the orientation the join carried in `params._screen`, and the
//! socket frames after the turn.
//!
//! Station `browser:display`, not `tests` (`scripts/gate_plan.py`,
//! `BROWSER_LOCKS`): `#[ignore]`d everywhere else. No node or no Chromium is a
//! `SKIP`, never a red (R2b).

#[path = "support/web_fixture.rs"]
mod web_fixture;

use meclaw_core::serde_json::json;
use std::time::Duration;
use web_fixture::backlog_lab::{MOUNT, start};

/// The value of `key=` in the driver's line.
fn field<'a>(line: &'a str, key: &str) -> &'a str {
    line.split_whitespace()
        .find_map(|p| p.strip_prefix(key))
        .unwrap_or_else(|| panic!("{key} is not in {line:?}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "browser lock: needs Chromium (workshop/tools/display-orientation-browser.mjs)"]
async fn gh1003_the_shell_marks_orientation_without_a_frame() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let driver = repo.join("workshop/tools/display-orientation-browser.mjs");
    if !driver.is_file() {
        println!("SKIP the driver does not ship in this tree");
        return;
    }
    let live = start(json!({"mount": MOUNT})).await;
    let run = tokio::time::timeout(
        Duration::from_secs(90),
        tokio::process::Command::new("node")
            .arg(&driver)
            .arg(format!("http://127.0.0.1:{}/{MOUNT}/", live.port))
            .kill_on_drop(true)
            .output(),
    )
    .await;
    let out = match run {
        Err(_) => panic!("the browser driver did not finish within 90 s"),
        Ok(Err(e)) => {
            println!("SKIP node is not on this host: {e}");
            return;
        }
        Ok(Ok(out)) => out,
    };
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    match out.status.code() {
        Some(3) => {
            println!("SKIP {}", stderr.trim());
            return;
        }
        Some(0) => {}
        other => panic!("the browser driver failed ({other:?}):\n{stdout}\n{stderr}"),
    }
    println!("{}", stdout.trim_end());
    let line = stdout
        .lines()
        .find(|l| l.starts_with("SCREEN "))
        .unwrap_or_else(|| panic!("no SCREEN line:\n{stdout}"));
    assert_eq!(field(line, "before="), "portrait", "upright: {line}");
    assert_eq!(
        field(line, "join_screen="),
        "portrait",
        "the join said so: {line}"
    );
    assert_eq!(field(line, "after="), "landscape", "turned: {line}");
    assert_eq!(field(line, "sent="), "0", "no frame to the server: {line}");
    assert_eq!(field(line, "received="), "0", "and none back: {line}");
    live.join.abort();
}
