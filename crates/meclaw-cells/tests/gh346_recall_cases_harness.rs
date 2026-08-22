//! GH #346 — the recall phase harness has a caller now, so it cannot go quiet
//! again.
//!
//! `workshop/evals/p5-longmemeval/tools/recall_cases.py` calls itself "the gate
//! of package P5: run this before every commit that touches recall". It was not
//! wired into CI and no test invoked it, so for several waves its silence was
//! indistinguishable from a pass: the stdin document had moved to the
//! post-envelope shape (GH #139), every case sent a context without provenance
//! and hit the audience gate, and the file finally died mid-run on a leg payload
//! whose shape had changed — with every assertion behind that point, including
//! the ones re-pointed at the payload/diagnostic split of #296, never executing.
//!
//! What the harness is worth pinning for: it drives the SHIPPED
//! `params.script_inline` out of `templates/memory-hive/recall/config.json`,
//! resolves the `${VAR:-default}` literals the colony would resolve, feeds it
//! hand-built messages on stdin and asserts the emitted multi-send. It tests the
//! shipped line rather than a copy of it — and it does so **offline**: no
//! colony, no store, no model, no embedder, no network, so it costs nothing and
//! can run on every build.
//!
//! This is the same shape `librarian_seed_corpus` uses for its generator: shell
//! out, relay the tool's own report, skip clean where the tool does not ship or
//! `python3` will not spawn.
//!
//! **The second test is the one that survives the next silence.** A harness can
//! stop measuring without failing — that is literally what #346 was — so
//! asserting the exit code alone would repeat the mistake at one remove: a file
//! whose cases stopped being registered exits 0 with nothing to say. So the
//! run's own report is read back, and a floor on the number of reported cases
//! makes a gutted harness red instead of green.

use std::path::PathBuf;
use std::process::{Command, Output};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../")
}

const HARNESS: &str = "workshop/evals/p5-longmemeval/tools/recall_cases.py";
/// The cell the harness drives. Named here as well so a tree that carries the
/// harness but not the template skips instead of failing on a dead reference.
const RECALL_CONFIG: &str = "templates/memory-hive/recall/config.json";

/// The line the harness prints when every case passed.
const ALL_PASS: &str = "ALL CASES PASS";

/// A floor, not a count. The harness reported 82 cases when this gate was
/// written; the floor is what makes a file that stopped REGISTERING cases red,
/// while leaving room for the cases the next recall change brings with it. If
/// this ever has to be lowered, the cases were deleted and that is the change
/// under review — never the floor.
const MIN_CASES: usize = 70;

/// The harness and the cell it drives, or `None` where this tree does not carry
/// them (the public export subset does not ship `workshop/`).
fn shipped() -> Option<PathBuf> {
    let root = repo_root();
    for rel in [HARNESS, RECALL_CONFIG] {
        if !root.join(rel).exists() {
            return None;
        }
    }
    Some(root)
}

/// Run the harness once. `None` means there is no interpreter on this machine.
fn run_harness() -> Option<Output> {
    let root = shipped()?;
    Command::new("python3")
        .arg(HARNESS)
        .current_dir(&root)
        .output()
        .ok()
}

/// The gate itself: every case the harness holds passes against the shipped
/// `recall` script.
///
/// The failure RELAYS rather than diagnoses (the lesson `librarian_seed_corpus`
/// records under GH #208): the harness names the case, prints what it got, and
/// the comment above that case names the contract sentence the assertion was
/// derived from. A headline of our own would sit above a report that already
/// says more.
#[test]
fn the_recall_phase_harness_passes_against_the_shipped_script() {
    let Some(out) = run_harness() else { return };

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "python3 {HARNESS} failed ({}). Its own report follows: each FAIL names the case, and \
         the comment above that case in the harness names the contract sentence the assertion \
         was derived from. Repair whichever of the two is wrong — never the assertion until it \
         agrees with the output, which is how this file went quiet in the first place (GH #346).\
         \n\n--- stdout (the harness report) ---\n{stdout}\
         \n--- stderr ---\n{stderr}",
        out.status,
    );
}

/// A green exit is not evidence of a measurement. GH #346 was a gate that passed
/// by not running, so this asks the harness what it actually did: the terminal
/// verdict line has to be there, and it has to sit behind a plausible number of
/// reported cases.
#[test]
fn the_harness_reports_the_cases_it_ran() {
    let Some(out) = run_harness() else { return };
    let stdout = String::from_utf8_lossy(&out.stdout);

    let cases = stdout.lines().filter(|l| l.starts_with("  PASS  ")).count();
    assert!(
        cases >= MIN_CASES,
        "{HARNESS} reported only {cases} passing cases, below the floor of {MIN_CASES}. A \
         harness that stops registering cases exits 0 with nothing to say, which is exactly the \
         silence GH #346 is about.\n\n--- stdout ---\n{stdout}",
    );
    assert!(
        stdout.contains(ALL_PASS),
        "{HARNESS} exited without printing {ALL_PASS:?}, so it did not reach its own verdict — a \
         section crashed in its setup, or the run was cut short.\n\n--- stdout ---\n{stdout}",
    );
}
