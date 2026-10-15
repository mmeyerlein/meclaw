//! GH #1002 T14: a phone on a slow link connects to a large page.
//!
//! Measured on a 2D city of about 4 400 objects: on Slow 3G the client gave
//! up three joins at 10.2 s each and never connected, because the join reply
//! was 0.69 MB in one frame. With the join in pieces of at most
//! `join_chunk_bytes`, the reply arrives first and the rest streams behind it.
//! Headless Chromium loads the city cold on two profiles through
//! `workshop/tools/display-slow3g-browser.mjs` and reports whether the container
//! connected, how many joins it sent, the largest frame, and whether the DOM
//! the pieces built equals the whole page.
//!
//! Outcome and state only, never duration (`docs/development-rules.md` § 11, Time
//! and Browser; GH #1199): the connect times the driver prints are measurements for
//! the record. Until wave Validierung 2 the test judged them against windows (26 s
//! Slow 3G, 6 s Fast 3G with CPU 4x), and under the coverage build's load the
//! Fast 3G connect took 6.2 s on build03 with every outcome green -- the window
//! judged the host. The failure this test exists for is an outcome: joins given
//! up, no connect, a frame over the limit, a page the pieces do not build.
//!
//! Station `browser:display`, not `tests` (`scripts/gate_plan.py`,
//! `BROWSER_LOCKS`): a browser lock runs where browser locks run and is
//! `#[ignore]`d everywhere else. No node or no Chromium is a `SKIP`, never a
//! red (R2b) -- outside the gate. Inside it the runner sets `MECLAW_REQUIRE_BROWSERS=1`
//! on a station that books the engine (GH #1161), and a Chromium that is absent or does
//! not start is `NO-ENGINE ...`, exit 1, red; only a missing node stays a `SKIP`.

#[path = "support/browser_run.rs"]
mod browser_run;
#[path = "support/gh1002_city.rs"]
mod city_fixture;

use city_fixture::{CHUNK, ENVELOPE, city, whole_of};
use meclaw_core::serde_json::json;
use meclaw_testing::deadline::{HANG, scaled};

/// The driver's run marker in HANG marks (two profiles, three waits each under their
/// own mark): 180 s at factor 1, three times the slowest whole run seen under load
/// (about 40 s in the coverage build on build03), and with the outside mark still
/// under the nextest leash of 240 s. A hang marker, never a budget.
const RUN_HANGS: u32 = 6;

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

    let mut cmd = tokio::process::Command::new("node");
    cmd.arg(&driver).arg(live.url("/")).arg(&expected);
    // Under the Rust marker over the whole child (`browser_run`): the driver gets the
    // run's HANG mark and names the wait it hung in; a child past the mark plus what
    // runs outside it is ended with its browser and reported as `HANG:`.
    let run = browser_run::run_marked(
        &mut cmd,
        "the slow-3G driver",
        scaled(HANG) * RUN_HANGS,
        scaled(HANG),
    )
    .await
    .unwrap_or_else(|hung| panic!("{hung}"));
    let out = match run {
        Err(e) => {
            println!("SKIP node is not on this host: {e}");
            return;
        }
        Ok(out) => out,
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
    for l in [line("slow3g"), line("fast3g_cpu4")] {
        // Before the pieces: on Slow 3G never connected, three joins given up.
        assert_eq!(
            field(&l, "connected="),
            1,
            "the container connects: {l}\n{stdout}"
        );
        assert_eq!(field(&l, "aborted="), 0, "no join is given up: {l}");
        assert_eq!(
            field(&l, "same_dom="),
            1,
            "the pieces build the whole page: {l}\n{stdout}"
        );
        assert!(
            field(&l, "largest=") <= (CHUNK + ENVELOPE) as i64,
            "no frame over the limit: {l}"
        );
    }
    live.join.abort();
}
