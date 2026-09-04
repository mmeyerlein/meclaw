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
//! **Reproducible is not complete (GH #344).** The regeneration gate above
//! compares the corpus with itself, and truncation is deterministic: a chunk
//! that lost half its section byte-matches its own regeneration forever. The
//! generator cut every section at `MAX_CHARS = 4000` with `body[:MAX_CHARS]`,
//! and when the overview's `Flags` section grew past that line, the paragraph
//! behind the cut — the one promising `--version`/`--help` write nothing —
//! stopped existing for the librarian with nothing going red. So the second
//! gate here reads the product and asks whether it still CONTAINS its sources:
//! no chunk sitting exactly on the cap (the signature of an arithmetic cut,
//! since the splitter always seams below it), and the lost paragraph present.
//!
//! **R2b guard.** Every read is guarded: where the generator or the corpus does
//! not ship, and where `python3` will not spawn, this skips rather than fails on
//! a dead reference.

use meclaw_core::serde_json::Value;
use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../")
}

const GENERATOR: &str = "workshop/tools/build_librarian_seed.py";
const CORPUS: &str = "templates/builder-librarian/store/seed/docs.jsonl";

/// The generator's cap, in characters. Mirrored, not imported — the point of
/// this gate is to fail if the two ever disagree about what a full chunk is.
const MAX_CHARS: usize = 4000;

/// The sentence GH #344 measured falling off the end of the `Flags` chunk. It
/// is quoted from `docs/meclaw-overview.en.md` — since GH #497 the generator
/// reads the ENGLISH edition of the four spec documents, because the lane that
/// searches this corpus briefs, asks and answers in English. The German
/// original still carries the same paragraph; it is simply not what is indexed.
const LOST_PARAGRAPH: &str = "Info-only flags are side-effect-free";

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
        // No interpreter on this machine. The `corpus` station of
        // `scripts/gate.sh` has one and runs the same command unguarded in
        // every strand, the integration pass and the release, so skipping here
        // loses coverage nowhere. (Not the public CI: the generator lives
        // under `workshop/`, which does not travel — GH #234.)
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

/// GH #344 — the corpus carries whole sections, not their first 4000 characters.
///
/// Two assertions, and the first is the general one: a chunk whose text is
/// exactly `MAX_CHARS` long was ended by arithmetic. The splitter seams on a
/// line break or a space and therefore lands strictly below the cap, so a chunk
/// sitting on it means somebody restored `body[:MAX_CHARS]` — the silent delete
/// this issue is about, which the regeneration gate cannot see because it is
/// perfectly reproducible.
///
/// The second names the casualty, so a regression reads as the loss it is
/// rather than as an arithmetic curiosity.
#[test]
fn no_chunk_is_a_silent_truncation() {
    let Some(root) = shipped() else { return };
    let corpus = std::fs::read_to_string(root.join(CORPUS)).expect("read corpus");

    let mut on_the_cap = Vec::new();
    let mut carries_the_paragraph = false;
    // Line 1 is the schema header, every line after it is a row.
    for line in corpus.lines().skip(1).filter(|l| !l.trim().is_empty()) {
        let row: Value = meclaw_core::serde_json::from_str(line).expect("corpus row is JSON");
        let text = row["text"].as_str().expect("every row has text");
        if text.chars().count() == MAX_CHARS {
            on_the_cap.push(format!(
                "{} ({} § {})",
                row["id"].as_str().unwrap_or("?"),
                row["source"].as_str().unwrap_or("?"),
                row["section"].as_str().unwrap_or("?"),
            ));
        }
        carries_the_paragraph |= text.contains(LOST_PARAGRAPH);
    }

    assert!(
        on_the_cap.is_empty(),
        "{} chunk(s) in {CORPUS} end exactly on the {MAX_CHARS}-character cap, which is what a \
         hard cut looks like: the section continues in the source and not in the corpus. The \
         generator must split an over-long section into `-cont<N>` rows, not truncate it.\n  {}",
        on_the_cap.len(),
        on_the_cap.join("\n  "),
    );
    assert!(
        carries_the_paragraph,
        "{CORPUS} does not contain {LOST_PARAGRAPH:?}. That paragraph is in \
         docs/meclaw-overview.en.md § Flags, past the 4000-character mark; if it is missing \
         here, the chunker is dropping section tails again (GH #344)."
    );
}
