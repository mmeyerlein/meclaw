//! GH #217 — the `builder-hive` template is a build product, and this is what
//! keeps it from lying.
//!
//! `templates/builder-hive/` is generated from readable python sources by
//! `workshop/tools/build_builder_hive.py` — the cell scripts live there as real
//! python because an inline script buried in JSON is unreviewable. Being
//! generated is not the same as being current: the tree is committed, so it can
//! hold cells that no longer match the sources they were built from.
//!
//! It already had, twice, and both were found by hand while fixing GH #215:
//! `main()` used to `shutil.rmtree` the whole output directory, which deleted
//! the hand-written `README.md` and `LIFT.md`; and the shipped `config.json`
//! carried `has(hop.*) &&` guards the generator's own `HIVE` did not, so any
//! regeneration reverted them silently. Those two reinforce each other — once
//! running the generator destroys hand-written prose, the safe move is to not
//! run it, and the drift then only grows.
//!
//! The gate is the shape GH #205 established for the librarian corpus:
//! regenerate, compare, fail with the command that repairs it. Two things are
//! specific to this one. It produces a TREE, so `--check` is a tree diff that
//! names the offending path — a cell renamed in `CELLS` leaves its old
//! directory behind, and "the tree differs" would not say where. And the
//! non-products (`README.md`, `LIFT.md`) are excluded from both the write path
//! and the comparison, because either inclusion breaks the gate in a different
//! direction: in the write path the generator eats them, in the comparison the
//! gate is red forever on files it never produced.
//!
//! **The failure relays, it does not diagnose (GH #208).** The generator exits
//! for two reasons that call for opposite fixes: drift is repaired by
//! regenerating and committing, a STRAY file survives a regeneration untouched
//! and has to be deleted by hand. Only the generator knows which one it saw, so
//! this test forwards its exit reason rather than prepending advice of its own.
//!
//! **R2b guard.** Both reads are guarded: where the generator or the template
//! does not ship, and where `python3` will not spawn, this skips rather than
//! fails on a dead reference. The `gates` job in `.github/workflows/ci.yml`
//! runs the same command with no skip path, so an interpreter-less runner
//! cannot make the gate quietly disappear.

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../")
}

const GENERATOR: &str = "workshop/tools/build_builder_hive.py";
const TREE: &str = "templates/builder-hive";
/// Hand-written prose that lives inside the generated tree. Named here as well
/// as in the generator so a change to one of the two lists shows up as a red
/// test rather than as a gate that silently stopped protecting them.
const NON_PRODUCTS: [&str; 2] = ["README.md", "LIFT.md"];

/// The generator and its product tree, or `None` where this tree does not carry
/// them.
fn shipped() -> Option<PathBuf> {
    let root = repo_root();
    for rel in [GENERATOR, TREE] {
        if !root.join(rel).exists() {
            return None;
        }
    }
    Some(root)
}

fn run_check(root: &Path) -> Option<std::process::Output> {
    Command::new("python3")
        .arg(GENERATOR)
        .arg("--check")
        .current_dir(root)
        .output()
        .ok()
}

/// Regenerate the template into a temp directory and diff the two trees. A
/// difference means somebody edited the shipped JSON by hand, or edited the
/// generator and did not rebuild what it generates.
#[test]
fn the_builder_hive_tree_matches_a_fresh_regeneration() {
    let Some(root) = shipped() else { return };
    // No interpreter on this machine. CI's `gates` job has one and runs the
    // same command unguarded, so skipping here loses coverage nowhere.
    let Some(out) = run_check(&root) else { return };

    // GH #208 — relay, do not guess. DRIFT is repaired by regenerating, a STRAY
    // file is not (the generator does not delete what it never made), and only
    // the generator's own message knows which of the two this is.
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "python3 {GENERATOR} --check failed ({}). What it exited with follows, \
         and it names both the offending path and the fix.\
         \n\n--- stderr (the exit reason) ---\n{stderr}\
         \n--- stdout (the gate's report) ---\n{stdout}",
        out.status,
    );
}

/// `--check` must not be a write in disguise. If it repaired the tree instead of
/// reporting on it, the test above would pass on every checkout and mean
/// nothing — which is the exact failure the gate exists to prevent, one level
/// up.
#[test]
fn the_check_mode_does_not_touch_the_committed_tree() {
    let Some(root) = shipped() else { return };
    let tree = root.join(TREE);

    let before = snapshot(&tree);
    if run_check(&root).is_none() {
        return;
    }
    let after = snapshot(&tree);

    assert_eq!(
        before, after,
        "{GENERATOR} --check rewrote files under {TREE}; check mode must only report"
    );
}

/// The prose the earlier `rmtree` deleted has to survive a real WRITE, not just
/// a `--check`. This is the regression the comparison cannot see by itself: a
/// diff that excludes `README.md` stays green while the generator removes it.
///
/// The write goes to a temp directory via `--out`, seeded with stand-ins at the
/// two non-product names. Running the real writer against the repository would
/// repair whatever drift the first test is asserting on, and a test that fixes
/// the tree it reports on proves nothing.
#[test]
fn a_regeneration_leaves_the_hand_written_prose_alone() {
    let Some(root) = shipped() else { return };

    let sandbox = std::env::temp_dir().join(format!(
        "meclaw-gh217-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&sandbox).expect("create sandbox");

    // Also plant a generated-looking directory, so the clearing the writer DOES
    // do stays visible: it is allowed to replace a cell it owns.
    std::fs::create_dir_all(sandbox.join("intake")).expect("create cell dir");
    std::fs::write(sandbox.join("intake/config.json"), b"{\"stale\": true}\n").expect("seed cell");
    for name in NON_PRODUCTS {
        std::fs::write(sandbox.join(name), format!("# {name}, written by hand\n"))
            .expect("seed prose");
    }

    let ran = Command::new("python3")
        .arg(GENERATOR)
        .arg("--out")
        .arg(&sandbox)
        .current_dir(&root)
        .output()
        .ok();
    let Some(out) = ran else {
        let _ = std::fs::remove_dir_all(&sandbox);
        return;
    };
    assert!(
        out.status.success(),
        "python3 {GENERATOR} --out failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let survivors: Vec<(&str, bool)> = NON_PRODUCTS
        .iter()
        .map(|name| {
            let body = std::fs::read_to_string(sandbox.join(name)).ok();
            (
                *name,
                body.as_deref() == Some(&format!("# {name}, written by hand\n")),
            )
        })
        .collect();
    let rebuilt = std::fs::read_to_string(sandbox.join("intake/config.json")).ok();
    let _ = std::fs::remove_dir_all(&sandbox);

    for (name, intact) in survivors {
        assert!(
            intact,
            "the generator removed or rewrote {name}. It is hand-written prose, not a \
             build product, and must be outside the write path (GH #215, GH #217)"
        );
    }
    assert_eq!(
        rebuilt.as_deref().map(str::trim_end),
        Some(build_product(&root, "intake/config.json").trim_end()),
        "the generator must still replace a cell directory it owns"
    );
}

/// One shipped product's bytes, for comparing against a sandbox write.
fn build_product(root: &Path, rel: &str) -> String {
    std::fs::read_to_string(root.join(TREE).join(rel)).expect("read shipped product")
}

/// A snapshot of every file under the tree, sorted, so a comparison is about
/// content and not about directory-walk order.
fn snapshot(tree: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![tree.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(body) = std::fs::read(&path) {
                let rel = path
                    .strip_prefix(tree)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, body));
            }
        }
    }
    out.sort();
    out
}
