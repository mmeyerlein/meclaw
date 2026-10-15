//! The colony half of the B-proofs: six runs, two engines, three exits (befund 04 § E.3).
//!
//! The file beside this one drives the SHEET -- a page the driver builds out of
//! `compose.py`'s own parts. That measures the engine, the sheet and the two hooks, and it
//! is all that fits in a gate. What it cannot measure is the screen: a gesture that has to
//! travel a socket and come back as a pass, a window the curator placed rather than a
//! fixture that was written down, a dock the pass filled, a second exit of the SAME state.
//! So this file boots the throwaway colony of befund 04 § E.2, fills its stage through the
//! app stand-in, and sends a real browser at each of the three exits in each of the two
//! engines:
//!
//! ```text
//! L1 chromium 1920x1080 monitor     L4 webkit 1920x1080 monitor
//! L2 chromium 3840x2160 tv          L5 webkit 3840x2160 tv
//! L3 chromium  393x852  phone       L6 webkit  393x852  phone
//! ```
//!
//! Which B-proofs a line carries is the exit's business (the driver's `RUNS`), and the
//! engine doubles it: § 6.7 and R-23-10 -- every browser on an iPhone is WebKit, and a
//! proof measured in Chromium alone says nothing about the device this screen is carried
//! on.
//!
//! **One test per line.** Each of the six lines is a test of its own with a colony of its
//! own: nextest runs a test as a process, so six tests can run side by side where one
//! test drove its six lines in a row. The Chromium lines do; the WebKit lines take turns
//! in the nextest group `webkit` (software rendering takes every core of the host), and
//! the names carry the engine so that the group's `test(/webkit/)` picks exactly those
//! three (`.config/nextest.toml`). Every line builds its stage on a fresh colony, so no
//! line measures what the gestures of the line before left behind.
//!
//! **Why `#[ignore]`.** Six boots, six pages, six sets of gestures: minutes, for proofs
//! that change between two strands about as often as the sheet does. The station
//! `browser:display` plans this with scope `colony` in `integration` and `release`
//! (GH #746) and runs it with `--run-ignored`; the sheet half carries the strand gate.
//!
//! **Where the evidence lands.** Screenshots and one report per run under
//! `target/beweis/display-browser/<engine>-<viewport>/`, so a red B-number can be looked
//! at rather than only read about. It used to land in the wave's own `plans/` folder,
//! which meant every release gate rewrote the pictures a finished wave had signed off on
//! and left the tree dirty -- and a dirty tree is a refused export (GH #775). Evidence a
//! wave keeps is committed once, by hand; what a run produces belongs under `target/`.
//!
//! **When SKIP and when RED.** Outside the gate, the same tool guard as every other
//! browser proof in this tree (R2b): no `playwright`, no browser bundle, no laboratory
//! (`wkenv.sh`), no `node` -- the driver says `SKIP` on a line of its own, leaves with 3,
//! and this test passes. A host that cannot measure is not a finding about the screen.
//! Inside the gate the station books the engines and the runner sets
//! `MECLAW_REQUIRE_BROWSERS=1` (GH #1161): an engine that is absent or does not start is
//! then `NO-ENGINE ...`, exit 1 and no report, and this test is red. Only a missing
//! `node` stays a `SKIP` there.

#[path = "support/display_colony.rs"]
mod display_colony;
// The verdict over one report lives beside the support of the colony, so that
// `710_the_colony_lines_verdict_reds_a_stalled_compositor.rs` can pin its behaviour
// on synthetic reports without a seventh test in this binary
// (re-check tv-bn-i M1; `scripts/tests/test_nextest_groups.py` holds it at six).
#[path = "support/colony_line.rs"]
mod colony_line;
// The Rust marker over the driver's child (orchestrator decision tv-bn-lb.2).

use std::path::{Path, PathBuf};

use colony_line::{Line, failures_of};
use display_colony::{
    Boot, Colony, Driver, Site, boot, browser_run, have_python, library_ships, repo,
};
use meclaw_core::serde_json::{Value, json};

const DRIVER: &str = "workshop/tools/display-layout-browser.mjs";
/// The laboratory WebKit runs in on this host, sourced for every run except a Chromium
/// one (the driver's default engine is WebKit). Sourced for Chromium too, a host without
/// the laboratory read its `SKIP` line as the Chromium run's own, and the lock proved
/// nothing there (XB-M review I-1: build02 skipped every Chromium run).
const WKENV: &str = "workshop/tools/wkenv.sh";
/// Where a run puts what its browsers saw. A WRITE, never a read: the guard above every
/// run is on the DRIVER, so in a tree without `workshop/` nothing is ever written here
/// either. Under `target/`, because a test writing into a committed folder makes the tree
/// dirty on every gate (GH #775).
const BEWEIS: &str = "target/beweis/display-browser";

/// The application whose windows stand on the stage for the whole file.
const APP: &str = "/alex/apps/probe";

const LINES: [Line; 6] = [
    Line {
        run: "L1",
        engine: "chromium",
        exit: "monitor",
    },
    Line {
        run: "L2",
        engine: "chromium",
        exit: "tv",
    },
    Line {
        run: "L3",
        engine: "chromium",
        exit: "phone",
    },
    Line {
        run: "L4",
        engine: "webkit",
        exit: "monitor",
    },
    Line {
        run: "L5",
        engine: "webkit",
        exit: "tv",
    },
    Line {
        run: "L6",
        engine: "webkit",
        exit: "phone",
    },
];

/// The stage every run looks at: nine ordinary windows, the conversation, a ringing timer
/// and a modal.
///
/// Written through the app stand-in and through the door, never into the state by hand:
/// what the browser then sees is what the curator placed, which is the whole point of the
/// colony half. Each `put`/`app_put` waits for ITS window to stand in the tree the patches
/// built at `web` -- a wait, not a pause: every write is its own pass on the one state in
/// the curator's memory (GH #809), so a write in the air beside another loses no window.
async fn fill_the_stage(colony: &Colony, round: u64) {
    let at = 1_700_000_000_000u64 + round * 100_000;
    // Sixteen and not nine: § 6.3 and § 6.2 ask what happens when MORE windows stand
    // open than fit, and nine of them fitted on a monitor with room to spare -- the
    // canvas never overflowed and two proofs had nothing to measure.
    for i in 1..=16 {
        colony
            .put(
                APP,
                &format!("w{i}"),
                json!({"title": format!("window {i}"), "context": "work",
                       "relevance": format!("0.{}", 9 - (i % 9)), "topic": format!("topic:{i}"),
                       "touched": (at + i).to_string()}),
            )
            .await;
    }
    // § 8.5: the conversation is a window of the modal ladder, pinned, with a turn of its
    // own -- and it is what B-26 to B-28 read. A REAL conversation: chat lines that each
    // name the channel they came through AND carry their moment (§ 8.4, R-26-1 -- the
    // support gives every line an `at` of its own), and an input line at the foot (§ 7.3).
    // Long enough to outgrow its window, so B-28 has a scroller to measure.
    let mut lines: Vec<Value> = Vec::new();
    for i in 1..=30 {
        lines.push(json!({
            "text": format!("line {i} of a conversation that is taller than its window, \
                             long enough that the box has to scroll"),
            "channel": if i % 3 == 0 { "phone" } else if i % 2 == 0 { "voice" } else { "typed" },
        }));
    }
    colony
        .app_put(
            json!({"do": "chat", "at": at + 100, "turn_id": "turn-1", "lines": lines,
                   "text": "the newest line of the conversation"}),
            "chat",
        )
        .await;
    // § 9.1: a timer that rings says `state: urgent` -- the front urgent of B-24. Its
    // tile carries `end_at` ten minutes out, so the seconds have something to run down
    // while the browser watches (D-27, B-21).
    let end_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("a clock after 1970")
        .as_millis() as u64
        + 600_000;
    colony
        .app_put(
            json!({"do": "ambient", "view": "timer", "at": at + 200,
                   "state": "urgent", "end_at": end_at, "text": "the egg is done"}),
            "timer",
        )
        .await;
    // § 6.3 asks what happens when MORE stands open than fits, "on every type" -- and
    // that is a question about HEIGHT, not about number: sixteen one-line windows fit on
    // a monitor with room to spare. Three windows of sixty paragraphs each do not fit
    // anywhere, so the canvas of every exit has something to scroll.
    // BELOW the conversation on purpose. A modal stands open only while it is the focus
    // window (§ 4.24), and the focus is the highest score; notes that outranked the chat
    // closed it before any proof could look at it (measured: B-26 red on three lines).
    // At 0.7 they are `relevant` -- open on the canvas -- and the conversation leads.
    for (n, rel) in [(1u8, "0.70"), (2, "0.69"), (3, "0.68")] {
        colony
            .app_put(
                json!({"do": "tall", "at": at + 400 + n as u64, "paragraphs": 60,
                       "view_id": format!("notes{n}"), "title": format!("Notes {n}"),
                       "topic": format!("notes:{n}"), "relevance": rel,
                       "text": format!("note {n}")}),
                &format!("notes{n}"),
            )
            .await;
    }
    // A second modal beside the conversation, so the plane blur of B-20 has a subject
    // even where the chat was closed by a turn of its own (§ 8.7).
    colony
        .put(
            APP,
            "sheetlet",
            // BELOW the conversation, like the notes: only ONE modal stands open (the
            // focus one, § 4.24), and a second one that outranks the chat closes it.
            json!({"title": "a modal moment", "layer": "modal", "context": "system",
                   "relevance": "0.6", "topic": "modal:1",
                   "touched": (at + 300).to_string()}),
        )
        .await;
}

/// Drive one line against the running colony, or `None` when this host cannot measure.
///
/// The driver goes through `sh` so that `wkenv.sh` is sourced first: Playwright's own
/// wrapper sets `LD_LIBRARY_PATH`, so the laboratory has to be in the environment of the
/// `node` process itself and cannot be handed over afterwards.
async fn drive(colony: &Colony, line: &Line, out_dir: &Path) -> Option<Value> {
    // Emptied first: the shots of a line that did not finish would otherwise be moved
    // under the NEXT line's viewport and stand there as its evidence (measured after a
    // run the harness killed mid-line).
    let _ = std::fs::remove_dir_all(out_dir);
    std::fs::create_dir_all(out_dir).expect("the evidence directory");
    // The driver on the line's exit, with the API: `MECLAW_DISPLAY_API` is where
    // `POST /messages` and `GET /colony/messages` answer -- NOT the page's own origin: that
    // port is the mount listener and answers 404 to every API route, and a proof that read
    // "nothing changed" off a 404 was green on an answer nobody gave (measured, all six
    // lines). The fixture puts it in (`Colony::drive`, killed on drop).
    let with = Driver::default()
        .script(DRIVER)
        .arg(out_dir)
        .args([
            "--engine",
            line.engine,
            "--profile",
            line.exit,
            "--run",
            line.run,
        ])
        // The SCREEN, not the app: a `in_view` is a write at the screen's door, and the
        // fixture's API stamps the app as its sender. Addressed to the app it reached a
        // `code` cell that had no command to read and nothing arrived (measured).
        .env("MECLAW_DISPLAY_NODE", display_colony::SCREEN);
    let cmd = colony.drive(Site::B710, line.exit, with);
    // Under the Rust marker: a child that outlives the driver's own marker plus what runs
    // outside it gets SIGTERM, then SIGKILL with its browser, and is reported as HANG
    // with its stderr tail before the harness kill that prints nothing (M1-R).
    let run = cmd
        .run_marked(
            &format!("the driver of line {} ({})", line.run, line.engine),
            browser_run::colony_run(),
            browser_run::outside(),
        )
        .await
        .unwrap_or_else(|hung| panic!("{hung}"));
    let out = match run {
        Ok(out) => out,
        Err(e) => {
            println!("SKIP neither node nor a shell for it on this host: {e}");
            return None;
        }
    };
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    // The driver's own SKIP, and only that: a plain `contains("SKIP")` would also match
    // the line Playwright prints whenever the laboratory is sourced, and a guard that
    // swallows every run inside the laboratory is a proof that cannot fail.
    if out.status.code() == Some(3) || stderr.lines().any(|l| l.starts_with("SKIP ")) {
        println!("SKIP {} {}: {}", line.run, line.engine, stderr.trim());
        return None;
    }
    // Exit 1 is a failing proof, not a reason to leave early: the report is on stdout
    // either way, and the names in it are what makes a red fixable.
    let text = stdout
        .lines()
        .find(|l| l.starts_with('{'))
        .unwrap_or_else(|| {
            panic!(
                "{} printed no report ({:?}):\n{stdout}\n{stderr}",
                line.run,
                out.status.code()
            )
        });
    std::fs::write(out_dir.join(format!("{}.json", line.run)), text).expect("the report");
    Some(meclaw_core::serde_json::from_str(text).expect("the driver's report is JSON"))
}

// NOT built: a crowd of forty windows, so that a grid exit runs out of canvas by the
// number of its rows (§ 6.3, OR-H0.10). It was written and measured, and it costs more
// than it proves: forty writes are forty passes, and the first write of the stage that
// followed them ran into a colony still working through the queue -- two runs, both
// dead at a thirty-second wait, with `settle` in between. The sentence "when the open
// windows do not fit, the canvas scrolls" is measured WHOLE on the stacked exit (the
// phone, where B-12 and B-30 run without a `partial`), and on a grid exit the proofs
// say in their `partial` why they cannot see it here: `max-content` rows and § 5.9's
// scroller inside every window mean such an exit overflows by NUMBER -- about three
// dozen open windows on a monitor -- and never by content.

/// Where one line's screenshots and its report are kept.
fn evidence_dir(engine: &str, viewport: &str) -> PathBuf {
    repo(BEWEIS).join(format!("{engine}-{viewport}"))
}

/// Boot a colony of its own, build the stage on it, and measure ONE line against it.
///
/// The SKIP paths are the ones the six lines had together: no template library or no
/// python, no driver in this tree, and the driver's own SKIP (no node, no playwright,
/// no engine, no laboratory) -- each one a green test that printed why it measured
/// nothing, never a red one (R2b).
async fn hold_the_line(run: &str) {
    let lines = &LINES;
    let line = lines
        .iter()
        .find(|l| l.run == run)
        .unwrap_or_else(|| panic!("{run} is not one of the lines"));
    // `MECLAW_710_LINES=L4,L5` keeps only the named lines; every other test of this
    // file says SKIP and measures nothing. For the targeted re-run of one flaky proof
    // (GH #1031: B-23 on L5, ten runs and more); `-E 'test(/webkit_on_the_tv/)'` does
    // the same through nextest. Unset, every line runs. A name that is no line, or an
    // empty value, is a typo and panics: filtered down to nothing the test would report
    // a green SKIP that measured no line at all.
    if let Ok(v) = std::env::var("MECLAW_710_LINES") {
        let names: Vec<String> = v.split(',').map(|s| s.trim().to_string()).collect();
        for n in &names {
            assert!(
                LINES.iter().any(|l| l.run == n.as_str()),
                "MECLAW_710_LINES={v:?}: {n:?} is not one of the lines {:?}",
                LINES.iter().map(|l| l.run).collect::<Vec<_>>()
            );
        }
        if !names.iter().any(|n| n == line.run) {
            println!("SKIP {}: not named in MECLAW_710_LINES={v:?}", line.run);
            return;
        }
    }
    if !library_ships() || !have_python() {
        println!("SKIP no template library or no python3 on this host");
        return;
    }
    if !repo(DRIVER).is_file() || !repo(WKENV).is_file() {
        println!("SKIP the browser driver does not ship in this tree");
        return;
    }

    // The judge stays out of the way: its verdict is the judge seam's proof
    // (`710_a_verdict_comes_back_from_the_judge.rs`), and a model answering in the middle
    // of a gesture is a second pass nobody asked for.
    //
    // Linger and fade far longer than the run. The shipped dials of this support are
    // small on purpose -- a lock that waits for a fade is a lock and not a nap -- but a
    // browser line takes a minute and more under load, and with `linger_ms` at two
    // seconds every window of the stage had gone `leaving` before the engine opened the
    // page. Measured: two windows of twelve still standing, and every proof that reads
    // the canvas measuring an empty one.
    let colony = boot(Boot {
        linger_ms: 900_000,
        fade_ms: 900_000,
        ..Boot::default()
    })
    .await;

    fill_the_stage(&colony, 1).await;
    // A waiting room of the line's own: the Chromium lines run side by side, and a
    // shared one would be emptied by the line next door (`drive` empties it first).
    // The viewport is the driver's own word for the exit, so the directory the shots
    // then move to is the one the report names.
    let pending = evidence_dir(line.engine, &format!("pending-{}", line.run));
    let Some(report) = drive(&colony, line, &pending).await else {
        let _ = std::fs::remove_dir_all(pending);
        colony.shutdown().await;
        println!(
            "SKIP {} {}: this host measured nothing",
            line.run, line.engine
        );
        return;
    };
    let viewport = report["viewport"].as_str().unwrap_or("?").to_string();
    let home = evidence_dir(line.engine, &viewport);
    std::fs::create_dir_all(&home).expect("the evidence directory");
    for entry in std::fs::read_dir(&pending).expect("read_dir") {
        let from = entry.expect("entry").path();
        let to = home.join(from.file_name().expect("a name"));
        std::fs::rename(&from, &to).expect("the evidence moves under its viewport");
    }
    let _ = std::fs::remove_dir_all(pending);

    colony.shutdown().await;

    if let Some(finding) = failures_of(line, &report) {
        panic!(
            "{} carries a B-proof that does not hold:\n\n{finding}",
            line.run
        );
    }
}

// integration + release, one boot and one page each. The station `browser:display` plans
// them with scope `colony` in `integration` and `release` (GH #746) and runs them with
// `--run-ignored`.

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "integration + release: one colony and one browser page (L1)"]
async fn the_colony_holds_in_chromium_on_the_monitor() {
    hold_the_line("L1").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "integration + release: one colony and one browser page (L2)"]
async fn the_colony_holds_in_chromium_on_the_tv() {
    hold_the_line("L2").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "integration + release: one colony and one browser page (L3)"]
async fn the_colony_holds_in_chromium_on_the_phone() {
    hold_the_line("L3").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "integration + release: one colony and one browser page (L4)"]
async fn the_colony_holds_in_webkit_on_the_monitor() {
    hold_the_line("L4").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "integration + release: one colony and one browser page (L5)"]
async fn the_colony_holds_in_webkit_on_the_tv() {
    hold_the_line("L5").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "integration + release: one colony and one browser page (L6)"]
async fn the_colony_holds_in_webkit_on_the_phone() {
    hold_the_line("L6").await;
}
