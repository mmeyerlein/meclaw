//! GH #230 — a corpus tree that states a substrate constraint names the
//! substrate's own verdict, not the process label of a ruling.
//!
//! `workshop/corpus/**` is a set of validated colony trees. Their `config.json`
//! and `template.json` files carry prose — script comments, `description`
//! slots — and that prose is read as configuration by whoever opens the
//! fixture next. One of those comments said `override_params` on a subtree
//! template "is R10-rejected". GH #140 replaced that reject with addressed
//! `override_params`; the comment outlived it by weeks and taught a
//! restriction that does not exist.
//!
//! **Why the citation form is the defect and not the wording.** `R10` is a
//! label from a process record — a corpus item's open question, answered in a
//! receipt. A process record is superseded silently: nothing about removing the
//! reject touches the label, so nothing can notice. The substrate's `error_code`
//! is the opposite kind of name: it exists in the colony's own vocabulary, it
//! is what a rejected mutation actually answers with, and it cannot be removed
//! without the removal being visible right here.
//!
//! So the rule this file enforces is a citation rule:
//!
//! 1. A sentence in a corpus TREE file that claims the substrate refuses
//!    something must name at least one **live** `MutationError::error_code`.
//! 2. Any token the corpus pins as an `error_code` must be live.
//!
//! "Live" is read from `MutationError::error_code` itself, so the vocabulary is
//! the substrate's and never a copy kept next to it. A code that is renamed or
//! deleted takes every corpus citation of it red in the same run.
//!
//! **What this gate deliberately does NOT do, and why.** It does not check that
//! the substrate still produces that code FOR THE FORM the sentence describes.
//! Doing so needs one executable probe per claim — a table of forms kept by
//! hand, which is the maintenance burden the ruling asked to avoid, and which
//! rots exactly like the comments it would guard. What is caught instead is the
//! whole class of citation that CANNOT be checked at all (a process label) and
//! the strongest form of "the rule is gone" (its verdict left the vocabulary).
//! A narrower gate that is true beats a wider one that is a decoration.
//!
//! **Scope: trees, not receipts.** `ITEM.md` and `RECEIPT.md` are records of a
//! run that happened, and a record is allowed to state what was true on its
//! day — the same reason the GH #229 sweep leaves `docs/archive/` alone. A
//! `config.json` is not a record; it is the thing the next reader configures
//! from.
//!
//! **The corpus is private.** In the published tree `workshop/` does not exist,
//! and an absent directory is skipped in silence rather than failed on — same
//! robustness rule as the shipped-docs sweep.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn core_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

// ────────────────────────────────────── the substrate's own verdict vocabulary

/// Where the colony defines what a rejected mutation answers with.
const ERROR_CODE_SOURCE: &str = "crates/meclaw-colony/src/mutation/mod.rs";

/// Every `error_code` the substrate can answer with — read out of the one function that
/// defines them, so this is the colony's vocabulary and not a copy of it.
fn live_error_codes() -> BTreeSet<String> {
    let src = std::fs::read_to_string(core_root().join(ERROR_CODE_SOURCE))
        .unwrap_or_else(|e| panic!("{ERROR_CODE_SOURCE}: {e}"));
    let body = src
        .split_once("pub fn error_code(&self) -> &'static str {")
        .unwrap_or_else(|| panic!("{ERROR_CODE_SOURCE}: error_code() moved or was renamed"))
        .1;
    let body = body
        .split_once("\n    }\n")
        .expect("error_code() body is not closed")
        .0;
    let mut out = BTreeSet::new();
    for arm in body.split("=> \"").skip(1) {
        if let Some((token, _)) = arm.split_once('"') {
            out.insert(token.to_string());
        }
    }
    assert!(
        out.len() >= 15,
        "the vocabulary reader found almost nothing: {out:?}"
    );
    out
}

/// Every snake_case token the shipped source spells as a string literal.
///
/// The wider vocabulary, and the one a PINNED code is held against: a corpus
/// tree cites cell verdicts (`mcp_error`, `provider_timeout`) next to mutation
/// verdicts, and those live as literals in the emitting cell rather than in one
/// enumerating function. `src/` only — a token that survives solely inside a
/// test is not something the substrate says to anyone.
fn live_code_literals() -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    collect_literals(&core_root().join("crates"), &mut out);
    assert!(
        out.len() >= 200,
        "the literal reader found almost nothing: {}",
        out.len()
    );
    out
}

fn collect_literals(dir: &Path, out: &mut BTreeSet<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let entry = entry.unwrap();
        let p = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if p.is_dir() {
            // `src/` only, but the crate directory above it has to be walked.
            if name != "tests" && name != "target" && name != "benches" {
                collect_literals(&p, out);
            }
            continue;
        }
        if p.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        if !p.components().any(|c| c.as_os_str() == "src") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&p) else {
            continue;
        };
        for part in text.split('"').skip(1).step_by(2) {
            if part.contains('_')
                && !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            {
                out.insert(part.to_string());
            }
        }
    }
}

// ─────────────────────────────────────────────── what a corpus tree writes down

/// The mutation surface a constraint sentence would be talking ABOUT.
///
/// Deliberately a closed list of names the substrate itself uses for diff
/// operations and their fields. A sentence that names none of them is not a
/// statement about the mutation surface, whatever else it says.
const MUTATION_SURFACE: &[&str] = &[
    "override_params",
    "add_nodes",
    "remove_nodes",
    "swap_nodes",
    "move_nodes",
    "add_edges",
    "remove_edges",
    "params.ports",
];

/// The words that turn a mention into a claim of refusal.
const REFUSAL: &[&str] = &[
    "reject",
    "refus",
    "block",
    "unsupported",
    "not allowed",
    "disallow",
    "forbidden",
];

/// One sentence of prose out of a corpus tree file.
struct Claim {
    file: String,
    text: String,
}

/// Every string value in a JSON document, flattened.
fn strings_in(v: &meclaw_core::JsonValue, out: &mut Vec<String>) {
    match v {
        meclaw_core::JsonValue::String(s) => out.push(s.clone()),
        meclaw_core::JsonValue::Array(a) => a.iter().for_each(|x| strings_in(x, out)),
        meclaw_core::JsonValue::Object(o) => o.values().for_each(|x| strings_in(x, out)),
        _ => {}
    }
}

/// Split prose into sentences: a `.` followed by whitespace ends one, and line
/// breaks are folded away first.
///
/// Folding the breaks is what makes the check hold: prose in a corpus tree is
/// wrapped by hand, and a claim wrapped between two lines would otherwise be
/// two halves, each harmless — the feature on one, the refusal on the other.
/// The defect this file was filed about happened to sit on a single line, and
/// a check that only works for that is a coincidence, not a gate. A `.` inside
/// code (`json.dumps(`) is not followed by whitespace and does not split.
fn sentences(text: &str) -> Vec<String> {
    let flat: Vec<char> = text
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .collect();
    let mut out = Vec::new();
    let mut cur = String::new();
    for (i, c) in flat.iter().enumerate() {
        cur.push(*c);
        if *c == '.' && flat.get(i + 1) == Some(&' ') {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

/// Every corpus TREE file, as `(relative path, its prose)`.
///
/// `.json` only: the machine-readable fixture is what a reader configures from.
/// Returns nothing at all when the corpus is not in this tree.
fn corpus_trees() -> Vec<(String, Vec<String>)> {
    let root = core_root();
    let mut out = Vec::new();
    walk(&root, &root.join("workshop/corpus"), &mut out);
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<String>)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return; // absent in the published tree — a subset is not a defect
    };
    for entry in entries {
        let entry = entry.unwrap();
        let p = entry.path();
        if p.is_dir() {
            walk(root, &p, out);
            continue;
        }
        if p.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&p) else {
            continue;
        };
        let Ok(v) = meclaw_core::serde_json::from_str::<meclaw_core::JsonValue>(&raw) else {
            continue; // a deliberately malformed negative fixture is not prose
        };
        let mut prose = Vec::new();
        strings_in(&v, &mut prose);
        out.push((p.strip_prefix(root).unwrap().display().to_string(), prose));
    }
}

/// Every sentence that claims the substrate refuses something.
fn constraint_claims(trees: &[(String, Vec<String>)], scanned: &mut usize) -> Vec<Claim> {
    let mut out = Vec::new();
    for (file, prose) in trees {
        for text in prose {
            for s in sentences(text) {
                *scanned += 1;
                let low = s.to_lowercase();
                if MUTATION_SURFACE.iter().any(|f| low.contains(f))
                    && REFUSAL.iter().any(|r| low.contains(r))
                {
                    out.push(Claim {
                        file: file.clone(),
                        text: s.trim().to_string(),
                    });
                }
            }
        }
    }
    out
}

/// Every token the corpus pins as an `error_code`, with the file it is in.
///
/// Literal by construction, like the `from:`/`to:` scan of GH #203: only the
/// word right after `error_code` is read. That phrasing IS a citation of the
/// substrate's vocabulary and there is nothing to guess about it.
fn pinned_codes(trees: &[(String, Vec<String>)]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (file, prose) in trees {
        for text in prose {
            for part in text.split("error_code").skip(1) {
                let token: String = part
                    .trim_start_matches([' ', ':', '=', '\'', '"', '`'])
                    .chars()
                    .take_while(|c| c.is_ascii_lowercase() || *c == '_')
                    .collect();
                if token.contains('_') {
                    out.push((file.clone(), token));
                }
            }
        }
    }
    out
}

// ───────────────────────────────────────────────────────────────── the checks

#[test]
fn a_corpus_constraint_names_a_verdict_the_substrate_still_gives() {
    let trees = corpus_trees();
    if trees.is_empty() {
        return; // published tree: the corpus is private and does not travel
    }
    let live = live_error_codes();
    let mut scanned = 0usize;
    let claims = constraint_claims(&trees, &mut scanned);

    let unbacked: Vec<String> = claims
        .iter()
        .filter(|c| !live.iter().any(|code| c.text.contains(code.as_str())))
        .map(|c| {
            format!(
                "{}: \"{}\" — this claims the substrate refuses something and names no \
                 error_code it answers with. A ruling label is a reference into a process \
                 record, and a process record is superseded without anything noticing; the \
                 verdict is the only name the colony still has to answer to.",
                c.file, c.text
            )
        })
        .collect();
    assert!(
        unbacked.is_empty(),
        "corpus trees state constraints nothing can check:\n  {}",
        unbacked.join("\n  ")
    );

    // "Nothing wrong" and "nothing read" look the same from here.
    assert!(
        scanned >= 500,
        "the sweep read almost no corpus prose: {scanned}"
    );
    assert!(
        !claims.is_empty(),
        "no corpus tree states a constraint any more — if that is real, this gate \
         has nothing left to guard and should be retired rather than kept green"
    );
}

#[test]
fn every_error_code_the_corpus_pins_is_one_the_substrate_still_has() {
    let trees = corpus_trees();
    if trees.is_empty() {
        return;
    }
    let live = live_code_literals();
    let dead: Vec<String> = pinned_codes(&trees)
        .into_iter()
        .filter(|(_, code)| !live.contains(code))
        .map(|(file, code)| {
            format!("{file}: pins error_code '{code}', which the substrate no longer has")
        })
        .collect();
    assert!(
        dead.is_empty(),
        "corpus trees pin verdicts that are gone:\n  {}",
        dead.join("\n  ")
    );
}

/// The test of the test: the vocabulary really is read from the substrate, and
/// the claim reader really recognises the shape this issue was filed about.
#[test]
fn the_reader_finds_the_claim_this_issue_was_filed_about() {
    let live = live_error_codes();
    for expected in ["schema", "hive_port_boundary", "resume_type_mismatch"] {
        assert!(live.contains(expected), "vocabulary is short: {live:?}");
    }

    let filed = vec![(
        "workshop/corpus/x/config.json".to_string(),
        vec![
            "# The unit prep offers a single v1-default tool (web_search) and \
             override_params on the subtree is R10-rejected, so the llm conveys the edit \
             through that tool's single 'query' string."
                .to_string(),
            "an add_nodes Resume at that occupied path must reject resume_type_mismatch \
             (pre-destructive, before any spawn)."
                .to_string(),
        ],
    )];
    let mut scanned = 0usize;
    let claims = constraint_claims(&filed, &mut scanned);
    assert_eq!(
        claims.len(),
        2,
        "{claims:#?}",
        claims = claim_texts(&claims)
    );

    let unbacked: Vec<&Claim> = claims
        .iter()
        .filter(|c| !live.iter().any(|code| c.text.contains(code.as_str())))
        .collect();
    assert_eq!(
        unbacked.len(),
        1,
        "exactly the R10 sentence is unbacked: {:#?}",
        claim_texts(&claims)
    );
    assert!(
        unbacked[0].text.contains("R10-rejected"),
        "{}",
        unbacked[0].text
    );
}

fn claim_texts(claims: &[Claim]) -> Vec<&str> {
    claims.iter().map(|c| c.text.as_str()).collect()
}
