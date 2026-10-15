//! GH #497 — the composer searches in English, so the corpus is English.
//!
//! The design lane's briefing is English, the composer answers in English, and
//! every query it has produced in a measured run is English. The corpus it
//! searches was German: the four spec documents were **294 of 590 chunks** —
//! over half of it, and the half that carries the specification — because the
//! generator globbed `docs/*.md` and the `.md` twin is the German original.
//!
//! `builder-librarian` searches with FTS5 and a lexical tokenizer. An English
//! query against German prose matches on the tokens the two languages happen to
//! share — `params.schema`, `script_inline`, `config.json` — and misses every
//! word that carries the meaning. The chunks that would have answered were in
//! the corpus; the sentences around their identifiers were not searchable.
//! Measured on one run, four `librarian_search` calls that each needed
//! `docs/cell-types.md` and none of which got it.
//!
//! So the generator reads the `.en.md` edition for both corpora
//! (`SPEC_DOCS` in `workshop/tools/build_librarian_seed.py`). The private
//! corpus labels a row with its REAL path, `docs/X.en.md`, because in this tree
//! that is the file a reader opens to find the sentence the row quotes; the
//! public corpus keeps labelling it `docs/X.md`, because that is the name the
//! exported tree carries for the same English bytes (GH #441, DOCS_MAP).
//!
//! Two assertions, in both directions, because only one of them alone can be
//! satisfied by an empty corpus:
//!
//!   * no `spec` row cites a German original — the switch is complete, not
//!     partial, and a fifth spec document added under its `.md` name would be
//!     caught here rather than diluting the corpus in silence;
//!   * a sentence that exists ONLY in the English edition is present, and its
//!     German twin is absent — the rows really did change language, rather than
//!     the label alone moving.
//!
//! **Both trees run this.** The corpus DOES ship: the export replaces it with a
//! public-only regeneration over the subset of sources that travel (R16 of
//! `make_export.py`). What differs is the LABEL, and it differs by exactly the
//! rename the export performs: in the private tree the English edition is
//! `docs/X.en.md`, in the published tree the same bytes are `docs/X.md` and no
//! German twin exists there at all. So "a German original" is a different set of
//! four names per tree, and the discriminator is the one thing that is only true
//! of the private tree: `docs/meclaw-overview.en.md` on disk. Reading the private
//! set in the published tree is what turned the 0.28.0 release CI red -- with all
//! 283 spec rows reported as German, in a tree that carries no German at all.
//! Where the corpus is absent entirely this skips rather than failing on a dead
//! reference.

use meclaw_core::serde_json::Value;
use std::path::PathBuf;

const CORPUS: &str = "templates/builder-librarian/store/seed/docs.jsonl";

/// The four spec documents, by the name their GERMAN original carries **in the
/// private tree**. A row citing one of these there is a row of German prose.
const GERMAN_ORIGINALS: [&str; 4] = [
    "docs/meclaw-overview.md",
    "docs/cell-types.md",
    "docs/config.md",
    "docs/rewiring.md",
];

/// The same four, by the name the ENGLISH edition carries in the private tree.
/// In the published tree these files do not exist -- the export publishes their
/// bytes under the plain names above -- so a row citing one there is a row
/// pointing at a file its own tree does not have.
const ENGLISH_EDITIONS: [&str; 4] = [
    "docs/meclaw-overview.en.md",
    "docs/cell-types.en.md",
    "docs/config.en.md",
    "docs/rewiring.en.md",
];

/// Which tree this is. Only the private one carries the `.en.md` editions beside
/// their German twins; the export ships one edition under one name.
fn private_tree() -> bool {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/meclaw-overview.en.md")
        .exists()
}

/// One sentence per language, from the same paragraph of the same section
/// (`meclaw-overview` § Flags). It is the paragraph GH #344 measured falling
/// off the end of a truncated chunk, so it is also the deepest one either
/// edition has to carry — a corpus that holds the English half holds the
/// section whole.
const ENGLISH_SENTENCE: &str = "Info-only flags are side-effect-free";
const GERMAN_SENTENCE: &str = "Info-only Flags sind side-effect-frei";

fn corpus() -> Option<String> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../")
        .join(CORPUS);
    std::fs::read_to_string(path).ok()
}

/// Every row of the corpus, schema header skipped.
fn rows(raw: &str) -> Vec<Value> {
    raw.lines()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|l| meclaw_core::serde_json::from_str(l).expect("corpus row is JSON"))
        .collect()
}

#[test]
fn no_spec_row_is_chunked_from_a_german_original() {
    let Some(raw) = corpus() else { return };

    // Two trees, two spellings of the same claim: the row must cite the English
    // edition under the name THIS tree gives it, and never the other one.
    let (wrong, right): (&[&str], &[&str]) = if private_tree() {
        (&GERMAN_ORIGINALS, &ENGLISH_EDITIONS)
    } else {
        (&ENGLISH_EDITIONS, &GERMAN_ORIGINALS)
    };

    let mut german = Vec::new();
    let mut english = 0usize;
    for row in rows(&raw) {
        if row["kind"].as_str() != Some("spec") {
            continue;
        }
        let source = row["source"].as_str().unwrap_or("?").to_string();
        if wrong.contains(&source.as_str()) {
            german.push(format!(
                "{} ({})",
                row["id"].as_str().unwrap_or("?"),
                source
            ));
        } else if right.contains(&source.as_str()) {
            english += 1;
        }
    }

    assert!(
        german.is_empty(),
        "{} of the corpus's `spec` rows cite the wrong edition for this tree \
         (private tree: {}):\n  {}\n\
         The lane that reads them briefs, asks and answers in English (GH #497). The \
         generator reads the `.en.md` edition — SPEC_DOCS in \
         workshop/tools/build_librarian_seed.py — and the export relabels it to the \
         plain name it publishes those bytes under (R16). A row on the other side of \
         that pair means a spec document was added under its German name, or the \
         public regeneration stopped relabelling.",
        german.len(),
        private_tree(),
        german.join("\n  "),
    );
    assert!(
        english > 0,
        "the corpus carries no `spec` row from the English edition at all. The \
         specification is over half of it; an empty half satisfies the assertion above \
         for the wrong reason."
    );
}

#[test]
fn the_english_edition_is_what_the_rows_hold() {
    let Some(raw) = corpus() else { return };

    let mut has_english = false;
    let mut german_rows = Vec::new();
    for row in rows(&raw) {
        let text = row["text"].as_str().unwrap_or("");
        has_english |= text.contains(ENGLISH_SENTENCE);
        if text.contains(GERMAN_SENTENCE) {
            german_rows.push(row["id"].as_str().unwrap_or("?").to_string());
        }
    }

    assert!(
        has_english,
        "the corpus does not contain {ENGLISH_SENTENCE:?}. It is in \
         docs/meclaw-overview.en.md § Flags, past the 4000-character mark, so its absence \
         means either the spec source reverted to the German original (GH #497) or the \
         chunker is dropping section tails again (GH #344)."
    );
    assert!(
        german_rows.is_empty(),
        "the corpus still carries the GERMAN twin of that paragraph, in row(s) {}. \
         Both editions in one corpus is the outcome GH #497 declined: it doubles the \
         spec rows and dilutes the ranking of every query that matches either.",
        german_rows.join(", "),
    );
}
