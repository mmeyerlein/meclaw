//! One line of the colony lock and the verdict over one driver report
//! (`710_the_colony_holds_in_both_engines_browser.rs`).
//!
//! Its own module because two binaries need it: 710 runs the six lines in a browser and
//! is held at exactly six tests (`scripts/tests/test_nextest_groups.py`), and
//! `710_the_colony_lines_verdict_reds_a_stalled_compositor.rs` pins the verdict's
//! behaviour on synthetic reports -- a compositor that never drew is red, a shot
//! that did not come back is an annotation (TV-B4). A pin on 710's source text let
//! three mutations through that turned a stalled compositor green (re-check tv-bn-i M1).
#![allow(dead_code)]

use meclaw_core::serde_json::Value;

/// One line of befund 04 § E.3: the run's name, the engine, and the exit it looks at.
pub struct Line {
    pub run: &'static str,
    pub engine: &'static str,
    pub exit: &'static str,
}

/// Every check of one report, or a line of failures for the caller to collect.
///
/// A `skipped: true` is allowed and printed by name -- a proof this mode cannot see at all
/// is honest about it. A failing check takes the whole `checks` block with it: a B-number
/// without the values beside it cannot be fixed, and the values are what tell a sheet
/// defect from a driver that measures the wrong thing.
pub fn failures_of(line: &Line, report: &Value) -> Option<String> {
    // A run the driver ended itself -- its run marker (`RUN_HANG_MS`, a `HANG:` naming
    // the proof it hung in), a signal, an error outside every proof -- says so in
    // `failed`, and that sentence is the red, not the proofs the run never reached.
    if let Some(why) = report["failed"].as_str() {
        return Some(format!(
            "{} ({}): {why} -- {}",
            line.run, line.engine, report["checks"]
        ));
    }
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
    // Two frames that never came before a shot (`frames_failed`, the driver's
    // `evidence()`) are printed by name and make the line red (post-xhigh-8 B3): a
    // compositor that does not draw is the screen's failure, found after every proof ran.
    let mut frames: Vec<String> = Vec::new();
    if let Some(stalled) = report["frames_failed"].as_object() {
        for (shot, why) in stalled {
            println!(
                "{} {} {shot}: no frames -- {}",
                line.run,
                line.engine,
                why.as_str().unwrap_or("")
            );
            frames.push(format!("frames {shot}"));
        }
    }
    // A picture that did not come back (`shots_not_taken`) is an annotation by name and
    // judges nothing (TV-B4; declared in re-check tv-bn-drv2-fix1 M, it replaced
    // post-xhigh-8 M2): the shot is evidence a person looks at, never a proof.
    let mut missing = 0usize;
    if let Some(lost) = report["shots_not_taken"].as_object() {
        for (shot, why) in lost {
            println!(
                "{} {} {shot}: shot not taken (annotation) -- {}",
                line.run,
                line.engine,
                why.as_str().unwrap_or("")
            );
            missing += 1;
        }
    }
    // A finger's tap is judged on the mark's own count (`report.taps`, the driver's
    // `fingerTap`, GH #1205); what Playwright's call did beside it -- returned after the
    // count moved, or never -- is printed as an ANNOTATION, never a verdict.
    if let Some(taps) = report.get("taps").filter(|t| !t.is_null()) {
        println!(
            "{} {} taps (annotation, no verdict): {taps}",
            line.run, line.engine
        );
    }
    println!(
        "{} {} {} {}: {} proofs, {half} of them half, {} of them failing, {} without frames, \
         {missing} shots not taken",
        line.run,
        line.engine,
        line.exit,
        report["viewport"].as_str().unwrap_or("?"),
        checks.len(),
        failed.len(),
        frames.len()
    );
    if failed.is_empty() && frames.is_empty() {
        return None;
    }
    let mut names: Vec<String> = failed.iter().map(|n| n.to_string()).collect();
    names.extend(frames);
    Some(format!(
        "{} ({} {}): {} -- {} {}",
        line.run,
        line.engine,
        report["viewport"].as_str().unwrap_or("?"),
        names.join(", "),
        meclaw_core::serde_json::to_string_pretty(&report["checks"]).expect("json"),
        report["frames_failed"]
    ))
}
