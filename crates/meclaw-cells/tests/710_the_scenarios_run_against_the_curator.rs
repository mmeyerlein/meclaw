//! The scenarios of the display hive against the real curator (display-hive.md § 11).
//!
//! The driver is Python and runs `compose.py`; this test runs the driver, so that the
//! pins stand in the nextest filter and every diff under `templates/display` pulls them
//! (`docs/development-rules.md` § 10). Three numbers have to be full: `MODEL` is the
//! document against itself (`run_model.py` against `pass.py`), `PURE` the scenarios
//! against the verbatim copy of the pass inside the cell, `CURATOR` the scenarios
//! through the cell's real entry -- two of them are colony-only and the driver says so
//! itself, which is why the third denominator is the smaller one.
//!
//! Both tests SKIP rather than fail where their material is legitimately absent: the
//! first when the library does not travel (R2b, the guard every tool-bound test in this
//! tree uses), the second when the private description is not there at all -- a foreign
//! clone and ci have the copies but not their source.

use std::process::Command;

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn home() -> String {
    std::env::var("HOME").unwrap_or_default()
}

const SCENARIOS: &str = "templates/display/compose/scenarios";
const DRIVER: &str = "templates/display/compose/scenarios/run_display_scenarios.py";

fn library_ships() -> bool {
    repo(DRIVER).exists()
}

/// `MODEL 109/109` -> `(109, 109)`. The driver writes the counter first and the
/// denominator second on a line of its own, one line per stage.
fn line(stdout: &str, prefix: &str) -> (u32, u32) {
    let rest = stdout
        .lines()
        .find_map(|l| l.strip_prefix(prefix))
        .unwrap_or_else(|| panic!("the driver printed no `{prefix}` line:\n{stdout}"));
    let (num, den) = rest
        .split_whitespace()
        .next()
        .and_then(|w| w.split_once('/'))
        .unwrap_or_else(|| panic!("`{prefix}` is no counter: {rest:?}"));
    (
        num.parse().expect("the counter"),
        den.parse().expect("the denominator"),
    )
}

#[test]
fn the_scenarios_run_against_the_curator() {
    if !library_ships() {
        println!("SKIP the_scenarios_run_against_the_curator: the library does not travel");
        return;
    }
    let out = Command::new("python3")
        .arg(repo(DRIVER))
        .output()
        .expect("the driver runs");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        out.status.success(),
        "the driver failed:\n{stdout}\n{stderr}"
    );
    for stage in ["MODEL ", "PURE ", "CURATOR "] {
        let (passed, total) = line(&stdout, stage);
        assert!(total > 0, "{stage}ran nothing:\n{stdout}");
        assert_eq!(passed, total, "{stage}is not full:\n{stdout}");
    }
}

/// The drift lock: the copy in the repo is byte for byte the description's model.
///
/// TWO STEPS, because the two readers ask different questions (GH #753).
///
/// A strand asks "is this commit's copy the commit it says it came from" and compares
/// against `SOURCE`, the mark beside the copies: `git show <sha>:./<file>` in the
/// living tree. That answer does not change while the gate runs. The integration and
/// release passes ask "is the copy the description as it stands today" and compare
/// against the working copy, as this lock always did -- that is the question a wave
/// has to answer before it ships, and it is the one the mark cannot answer.
///
/// Splitting them is paid for: on one evening three sessions moved the living
/// description while strand gates ran against it, and six full strand gates went red
/// on this one test -- 5589 gate seconds, none of them about the strand's own work
/// (`plans/welle-p-2026-09-19/befund/01-zeit.md` section 3.2).
///
/// The runner names the mode in `MECLAW_GATE_MODE`; nothing set means a strand, so a
/// test run by hand gets the deterministic half. Without the living tree (ci, a
/// foreign clone) both halves SKIP with their reason on stdout -- the same guard the
/// WebKit laboratory uses.
///
/// ONE more silence, and only one: a tree that does not know the mark's commit yet.
/// A missing or malformed mark is RED, with the same sentence a drift gets -- the
/// mark travels with the copies, so its absence is a defect here, and a SKIP would
/// make deleting it the way around the lock (wave P review M3). The three copies are compared as whole files: they are
/// taken over unchanged, `sync_pass.py` only carries `pass.py` on into `compose.py`
/// and never edits it, so nothing here may differ -- not even a docstring.
#[test]
fn the_copy_is_the_document() {
    let src = std::env::var("MECLAW_DISPLAY_HIVE")
        .unwrap_or_else(|_| format!("{}/projeks/MeClaw/meclaw-next/23-display", home()));
    if !std::path::Path::new(&src).is_dir() || !library_ships() {
        println!("SKIP the_copy_is_the_document: the description tree is not here");
        return;
    }
    let model = format!("{src}/model");
    let mode = std::env::var("MECLAW_GATE_MODE").unwrap_or_default();
    // `<rev>:./<file>` resolves against the working directory inside the repository,
    // so the model's own path never has to be spelled out here.
    let mark = match mode.is_empty() || mode == "strand" {
        false => None, // integration and release ask the working copy
        true => match mark_sha() {
            // A MISSING or unreadable mark is a defect of THIS repository, not a
            // clone that lags behind: the mark travels with the copies, and every
            // tree that has the one has the other. Skipping here would let a strand
            // that bends the copies and deletes the mark stay green in every strand
            // gate -- the SKIP would hide the very drift the lock exists for
            // (wave P review M3).
            None => panic!(
                "{SCENARIOS}/SOURCE names no commit (display-hive.md § 0.7); \
                 run `python3 scripts/display_sync.py` to renew copies and mark"
            ),
            // The one silence a clone may legitimately produce: it was fetched
            // before the description moved, so it cannot resolve the mark at all.
            Some(sha) if !commit_exists(&model, &sha) => {
                println!("SKIP the_copy_is_the_document: the description tree has no {sha} yet");
                return;
            }
            some => some,
        },
    };
    for name in ["scenarios.json", "pass.py", "run_model.py"] {
        // The driver's own name for the model's runner; the other two keep theirs.
        let source_name = if name == "run_model.py" {
            "run.py"
        } else {
            name
        };
        let (there, whence) = match mark.as_deref() {
            Some(sha) => (
                show(&model, sha, source_name),
                format!("the model at {sha} (SOURCE)"),
            ),
            None => (
                std::fs::read(format!("{model}/{source_name}"))
                    .unwrap_or_else(|e| panic!("the source {source_name}: {e}")),
                "the description's working copy".to_string(),
            ),
        };
        let here = std::fs::read(repo(&format!("{SCENARIOS}/{name}")))
            .unwrap_or_else(|e| panic!("the copy {name}: {e}"));
        // `assert!`, not `assert_eq!`: a diff of 169 kB of JSON is no message.
        assert!(
            here == there,
            "{name} differs from {whence} (display-hive.md § 0.7); \
             run `python3 scripts/display_sync.py` to renew copies and mark"
        );
    }
}

/// The commit named by the mark `SOURCE`: one line, `meclaw-next <sha> <date>`.
fn mark_sha() -> Option<String> {
    let text = std::fs::read_to_string(repo(&format!("{SCENARIOS}/SOURCE"))).ok()?;
    let sha = text.split_whitespace().nth(1)?.to_string();
    (sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit())).then_some(sha)
}

/// Whether the living tree already carries that commit. It may not: a clone fetched
/// before the description moved is behind, and being behind is not a drift.
fn commit_exists(model: &str, sha: &str) -> bool {
    Command::new("git")
        .args(["-C", model, "cat-file", "-e", &format!("{sha}^{{commit}}")])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// One file of the model as that commit holds it.
fn show(model: &str, sha: &str, name: &str) -> Vec<u8> {
    let out = Command::new("git")
        .args(["-C", model, "show", &format!("{sha}:./{name}")])
        .output()
        .unwrap_or_else(|e| panic!("git show {sha}:./{name}: {e}"));
    assert!(
        out.status.success(),
        "git show {sha}:./{name} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}
