//! GH #205 — the `builder-librarian` seed corpus is a build product, and this
//! is what keeps it from lying.
//!
//! `templates/builder-librarian/store/seed/docs.jsonl` is generated from the
//! spec, the cookbook, the corpus briefs, the template catalogue and the pinned
//! error codes by `workshop/tools/build_librarian_seed.py`. Being generated is
//! not the same as being current: the file is committed, so the tree can hold a
//! corpus that describes a tree that no longer exists. It did, for 289 lines —
//! `memory-hive@1.2.0`, `collector@1.2.0` and `talky@1.2.0` after all three had
//! moved, and eight templates missing outright.
//!
//! That is worse than an empty corpus. The librarian answers "what templates
//! exist and what do they do", and BM25 ranks a stale answer exactly as high as
//! a true one, so nothing downstream can tell them apart.
//!
//! The gate is the same shape as the one on `scripts/canvy_sync.py`: regenerate,
//! byte-compare, fail on any difference — the corpus is current or the build is
//! red, with no third state. The generator does the comparison (`--check`); this
//! test is one of the two places it is invoked from. The other is the `gates`
//! job in `.github/workflows/ci.yml`, which runs it without a skip path, so a
//! machine with no `python3` cannot make the gate quietly disappear.
//!
//! **The failure relays, it does not diagnose (GH #208).** The generator exits
//! for two reasons and they call for opposite fixes: drift is repaired by
//! regenerating and committing, while a source that is present and does not
//! parse (GH #207) is repaired in the source — a regeneration reproduces the
//! same failure byte for byte. So this test forwards the generator's own exit
//! reason instead of prepending "regenerate", which was advice that visibly did
//! nothing for one of the two cases and sat one line above the generator saying
//! so.
//!
//! **R2b guard.** Both reads are guarded: where the generator or the corpus does
//! not ship, and where `python3` will not spawn, this skips rather than fails on
//! a dead reference.

use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../")
}

const GENERATOR: &str = "workshop/tools/build_librarian_seed.py";
const CORPUS: &str = "templates/builder-librarian/store/seed/docs.jsonl";

/// The generator and its product, or `None` where this tree does not carry them.
fn shipped() -> Option<PathBuf> {
    let root = repo_root();
    for rel in [GENERATOR, CORPUS] {
        if !root.join(rel).exists() {
            return None;
        }
    }
    Some(root)
}

/// Regenerate the corpus into a temp file and byte-compare it against the
/// committed one. A difference means somebody changed a source the librarian
/// describes and did not rebuild what describes it.
#[test]
fn the_seed_corpus_matches_a_fresh_regeneration() {
    let Some(root) = shipped() else { return };

    let out = match Command::new("python3")
        .arg(GENERATOR)
        .arg("--check")
        .current_dir(&root)
        .output()
    {
        Ok(out) => out,
        // No interpreter on this machine. CI's `gates` job has one and runs the
        // same command unguarded, so skipping here loses coverage nowhere.
        Err(_) => return,
    };

    // GH #208 — relay, do not guess. The generator exits for two reasons that
    // call for opposite fixes, and only it knows which one this is: drift is
    // repaired by regenerating and committing, a source that is present and
    // does not parse is repaired in the source and survives a regeneration
    // untouched. A headline of our own would be right half the time and would
    // sit directly above the generator's own line contradicting it.
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "python3 {GENERATOR} --check failed ({}). What it exited with follows, \
         and it names both the reason and the fix.\
         \n\n--- stderr (the exit reason) ---\n{stderr}\
         \n--- stdout (the gate's report) ---\n{stdout}",
        out.status,
    );
}

/// `--check` must not be a write in disguise. If it repaired the file instead of
/// reporting on it, the test above would pass on every tree and mean nothing —
/// which is the exact failure the gate exists to prevent, one level up.
#[test]
fn the_check_mode_does_not_touch_the_committed_corpus() {
    let Some(root) = shipped() else { return };
    let corpus = root.join(CORPUS);

    let before = std::fs::read(&corpus).expect("read corpus");
    let ran = Command::new("python3")
        .arg(GENERATOR)
        .arg("--check")
        .current_dir(&root)
        .output()
        .is_ok();
    if !ran {
        return;
    }
    let after = std::fs::read(&corpus).expect("read corpus");

    assert_eq!(
        before, after,
        "{GENERATOR} --check rewrote {CORPUS}; check mode must only report"
    );
}
