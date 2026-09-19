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
//! **Why `#[ignore]`.** Six boots, six pages, six sets of gestures: four to six minutes,
//! a fifth of the integration pass's whole budget, for proofs that change between two
//! strands about as often as the sheet does. The station `browser:display` plans this
//! with scope `colony` in `release` only and runs it with `--run-ignored`; the sheet half
//! carries the gate.
//!
//! **Where the evidence lands.** Screenshots and one report per run under
//! `plans/welle-h-2026-09-17/beweis/browser/<engine>-<viewport>/`, so a red B-number can
//! be looked at rather than only read about.
//!
//! **Why SKIP and not RED.** The same tool guard as every other browser proof in this
//! tree (R2b): no `playwright`, no browser bundle, no laboratory (`wkenv.sh`), no `node`
//! -- the driver says `SKIP` on a line of its own, leaves with 3, and this test passes. A
//! host that cannot measure is not a finding about the screen.

#[path = "support/display_colony.rs"]
mod display_colony;

use std::path::{Path, PathBuf};

use display_colony::{Boot, Colony, boot, have_python, library_ships, repo};
use meclaw_core::serde_json::{Value, json};

const DRIVER: &str = "workshop/tools/display-layout-browser.mjs";
/// The laboratory WebKit runs in on this host. Sourced for BOTH engines, so a run has one
/// shape: Chromium ignores every variable in it (OR-H5.3).
const WKENV: &str = "workshop/tools/wkenv.sh";
/// Where the wave keeps what its browsers saw. A WRITE, never a read: the guard above
/// every run is on the DRIVER, so in a tree without `workshop/` nothing is ever written
/// here either.
const BEWEIS: &str = "plans/welle-h-2026-09-17/beweis/browser";

/// The application whose windows stand on the stage for the whole file.
const APP: &str = "/alex/apps/probe";

/// One line of befund 04 § E.3: the run's name, the engine, and the exit it looks at.
struct Line {
    run: &'static str,
    engine: &'static str,
    exit: &'static str,
}

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
/// colony half. Each `put`/`app_put` waits for ITS window to reach the state row -- a wait,
/// not a pause: a pass reconciles its state with the store's rows before its own event runs
/// (§ 3.1, OR-H0.9), so a write in the air beside another loses no window.
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
async fn drive(line: &Line, url: &str, api: &str, out_dir: &Path) -> Option<Value> {
    // Emptied first: the shots of a line that did not finish would otherwise be moved
    // under the NEXT line's viewport and stand there as its evidence (measured after a
    // run the harness killed mid-line).
    let _ = std::fs::remove_dir_all(out_dir);
    std::fs::create_dir_all(out_dir).expect("the evidence directory");
    let run = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(". \"$1\"; shift; exec node \"$@\"")
        .arg("sh")
        .arg(repo(WKENV))
        .arg(repo(DRIVER))
        .arg(url)
        .arg(out_dir)
        .arg("--engine")
        .arg(line.engine)
        .arg("--profile")
        .arg(line.exit)
        .arg("--run")
        .arg(line.run)
        // The node process holds a browser. A dropped future does not take a child with
        // it, so the browser would outlive the test that started it.
        .kill_on_drop(true)
        // The SCREEN, not the app: a `in_view` is a write at the screen's door, and the
        // fixture's API stamps the app as its sender. Addressed to the app it reached a
        // `code` cell that had no command to read and nothing arrived (measured).
        .env("MECLAW_DISPLAY_NODE", display_colony::SCREEN)
        // Where `POST /messages` and `GET /colony/messages` answer. NOT the page's own
        // origin: that port is the mount listener and answers 404 to every API route --
        // and a proof that read "nothing changed" off a 404 was green on an answer
        // nobody gave (measured, all six lines).
        .env("MECLAW_DISPLAY_API", api)
        .output()
        .await;
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

/// Every check of one report, or a line of failures for the caller to collect.
///
/// A `skipped: true` is allowed and printed by name -- a proof this mode cannot see at all
/// is honest about it. A failing check takes the whole `checks` block with it: a B-number
/// without the values beside it cannot be fixed, and the values are what tell a sheet
/// defect from a driver that measures the wrong thing.
fn failures_of(line: &Line, report: &Value) -> Option<String> {
    let checks = report["checks"]
        .as_object()
        .unwrap_or_else(|| panic!("{}: the report carries no checks: {report}", line.run));
    assert!(
        !checks.is_empty(),
        "{}: the line measured nothing at all: {report}",
        line.run
    );
    let mut failed: Vec<&str> = Vec::new();
    let mut half = 0usize;
    for (name, check) in checks {
        if check["skipped"] == Value::Bool(true) {
            println!(
                "{} {} {name}: skipped -- {}",
                line.run,
                line.engine,
                check["why"].as_str().unwrap_or("")
            );
        }
        // A proof that measured only half of its sentence says so in `value.partial`.
        // Printed by name, because a half measurement that only stands in a JSON file
        // nobody reads in the gate looks exactly like a whole one.
        if let Some(why) = check["value"].get("partial").and_then(Value::as_str) {
            println!("{} {} {name}: half -- {why}", line.run, line.engine);
            half += 1;
        }
        if check["ok"] != Value::Bool(true) {
            failed.push(name);
        }
    }
    println!(
        "{} {} {} {}: {} proofs, {half} of them half, {} of them failing",
        line.run,
        line.engine,
        line.exit,
        report["viewport"].as_str().unwrap_or("?"),
        checks.len(),
        failed.len()
    );
    if failed.is_empty() {
        return None;
    }
    Some(format!(
        "{} ({} {}): {} -- {}",
        line.run,
        line.engine,
        report["viewport"].as_str().unwrap_or("?"),
        failed.join(", "),
        meclaw_core::serde_json::to_string_pretty(&report["checks"]).expect("json")
    ))
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
// release-only: 4-6 min. Six boots, six pages, six sets of gestures -- a fifth of the
// integration pass's budget. The station `browser:display` plans it with scope `colony`
// in `release` and runs it with `--run-ignored`.
#[ignore = "release-only: 4-6 min"]
async fn the_colony_holds_in_both_engines() {
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
    // small on purpose -- a lock that waits for a fade is a lock and not a nap -- but six
    // browser lines take a minute and more, and with `linger_ms` at two seconds every
    // window of the stage had gone `leaving` before the first engine opened the page.
    // Measured: two windows of twelve still standing, and every proof that reads the
    // canvas measuring an empty one.
    let colony = boot(Boot {
        linger_ms: 900_000,
        fade_ms: 900_000,
        ..Boot::default()
    })
    .await;

    let api = colony.api_url();
    let mut findings: Vec<String> = Vec::new();
    let mut measured = 0usize;
    let mut round = 0u64;
    for line in &LINES {
        // The stage is built again for EVERY line. A tap is a real dismissal and the
        // state it changes is the colony's: the gesture proofs of the line before leave
        // windows put away, and the line after them would then measure a screen nobody
        // built. Measured: after the three Chromium lines the conversation stood at
        // level 0, and every WebKit proof about an open modal had no modal to look at.
        // Every round carries its OWN `touched`: a write that repeats the touch of the
        // one before it is a repeated write and lifts nothing (§ 4.8), so a stage
        // rebuilt with the old numbers would leave the put-away standing.
        round += 1;
        fill_the_stage(&colony, round).await;
        let url = colony.url(line.exit);
        // The viewport is the driver's own word for the exit, so the directory a shot
        // lands in is the one the report names.
        let Some(report) = drive(line, &url, &api, &evidence_dir(line.engine, "pending")).await
        else {
            continue;
        };
        let viewport = report["viewport"].as_str().unwrap_or("?").to_string();
        let home = evidence_dir(line.engine, &viewport);
        std::fs::create_dir_all(&home).expect("the evidence directory");
        for entry in std::fs::read_dir(evidence_dir(line.engine, "pending")).expect("read_dir") {
            let from = entry.expect("entry").path();
            let to = home.join(from.file_name().expect("a name"));
            std::fs::rename(&from, &to).expect("the evidence moves under its viewport");
        }
        measured += 1;
        if let Some(finding) = failures_of(line, &report) {
            findings.push(finding);
        }
    }
    let _ = std::fs::remove_dir_all(evidence_dir("chromium", "pending"));
    let _ = std::fs::remove_dir_all(evidence_dir("webkit", "pending"));

    colony.shutdown().await;

    if measured == 0 {
        println!("SKIP no engine on this host measured anything");
        return;
    }
    assert!(
        findings.is_empty(),
        "{} of {measured} lines carry a B-proof that does not hold:\n\n{}",
        findings.len(),
        findings.join("\n\n")
    );
}
