//! GH #1002 T14: a phone on a slow link connects to a large page.
//!
//! Measured on a 2D city of about 4 400 objects: on Slow 3G the client gave
//! up three joins at 10.2 s each and never connected, because the join reply
//! was 0.69 MB in one frame. With the join in pieces of at most
//! `join_chunk_bytes`, the reply arrives first and the rest streams behind it.
//! Headless Chromium loads the city cold on two profiles through
//! `workshop/tools/display-slow3g-browser.mjs` and reports when the container
//! connected, how many joins it sent, the largest frame, and whether the DOM
//! the pieces built equals the whole page.
//!
//! Station `browser:display`, not `tests` (`scripts/gate_plan.py`,
//! `BROWSER_LOCKS`): a browser lock runs where browser locks run and is
//! `#[ignore]`d everywhere else. No node or no Chromium is a `SKIP`, never a
//! red (R2b).

#[path = "support/gh1002_city.rs"]
mod city_fixture;

use city_fixture::{CHUNK, ENVELOPE, city, whole_of};
use meclaw_core::serde_json::json;
use std::time::Duration;

/// The value of `key=` in a driver line.
fn field(line: &str, key: &str) -> i64 {
    line.split_whitespace()
        .find_map(|p| p.strip_prefix(key))
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| panic!("{key} is not in {line:?}"))
}

/// T14: a phone on Slow 3G joins the city, and the pieces build the page.
///
/// The profiles are the city measurement's
/// (`workshop/tools/display-slow3g-browser.mjs` names them).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "browser lock: needs Chromium (workshop/tools/display-slow3g-browser.mjs)"]
async fn gh1002_slow_3g_connects_and_no_join_is_given_up() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let driver = repo.join("workshop/tools/display-slow3g-browser.mjs");
    if !driver.is_file() {
        println!("SKIP the driver does not ship in this tree");
        return;
    }
    let (td, live) = city(json!({})).await;
    let (_, body) = whole_of(&td.path().join("web"));
    let expected = td.path().join("expected.html");
    std::fs::write(&expected, &body).expect("expected body");

    let run = tokio::time::timeout(
        Duration::from_secs(240),
        tokio::process::Command::new("node")
            .arg(&driver)
            .arg(live.url("/"))
            .arg(&expected)
            .kill_on_drop(true)
            .output(),
    )
    .await;
    let out = match run {
        Err(_) => panic!("the browser driver did not finish within 240 s"),
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
    let line = |p: &str| {
        stdout
            .lines()
            .find(|l| l.contains(&format!("profile={p} ")))
            .unwrap_or_else(|| panic!("no line for {p}:\n{stdout}"))
            .to_string()
    };
    let slow = line("slow3g");
    let fast = line("fast3g_cpu4");
    for l in [&slow, &fast] {
        assert_eq!(field(l, "aborted="), 0, "no join is given up: {l}");
        assert_eq!(
            field(l, "same_dom="),
            1,
            "the pieces build the whole page: {l}"
        );
        assert!(
            field(l, "largest=") <= (CHUNK + ENVELOPE) as i64,
            "no frame over the limit: {l}"
        );
    }
    // Slow 3G: measured 13.0 s to `phx-connected`, one join, 0 aborted (before:
    // never, three joins aborted at 10.2 s). The plan's 10 s is not reachable
    // on this profile by any join: 2 000 ms latency per round trip times page,
    // scripts, stylesheet, socket upgrade and join is ≥ 10 s before one byte of
    // tree (OR-H4.W2.3). The limit is the measured number with a factor 2.
    assert!(
        (0..=26_000).contains(&field(&slow, "connected_ms=")),
        "Slow 3G connects: {slow}"
    );
    assert!(
        (0..=6_000).contains(&field(&fast, "connected_ms=")),
        "Fast 3G with CPU 4x connects within 6 s: {fast}"
    );
    live.join.abort();
}
