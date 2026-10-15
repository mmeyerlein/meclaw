//! GH #26 — an app-hive name taken from the substrate glossary is a
//! template-review defect, and this is the review that never forgets.
//!
//! **Where the rule comes from.** GH #26 (the naming epic) has named the fork
//! for months: `registry` and `scheduler` are words the substrate already owns —
//! the templates registry a mutation resolves against, and the scheduling the
//! `timer` cell type does — so a *level content* carrying one of them makes two
//! different things answer to one word. It was ruled on 2026-08-21 as **Q19**
//! (ruling Q19): rename now —
//! **`catalog` not `registry`, `calendar` not `scheduler`** — and record that a
//! glossary collision is a defect at template review, not a preference. The
//! declined branch would have kept both words and left a "prefer, not forbid"
//! sentence in `docs/development-rules.md` with nothing reading it.
//!
//! **Why a directory name is the thing under test.** A template's subdirectory
//! name is not decoration: `parse_subtree` derives each cell's `rel_path` from
//! the directory name (`relative_path`, called per directory in the collecting
//! walk), and that rel-path becomes the address segment the grown instance is
//! reachable at. A directory called `registry` inside a level template IS a node
//! called `registry` in every tree grown from it — which is the *expensive
//! later* the epic warns about, because after the first instance is grown the
//! name lives in edges, in operator habits and in whatever a person typed into a
//! mutation. The name is free to change while it is only a directory.
//!
//! **Deliberately narrow, and the narrowness is asserted, not merely stated.**
//! The deny-list is exactly the two words Q19 ruled on. `store`, `collector`,
//! `dispatcher` and `timer` are cell-type and role words that shipped templates
//! use *correctly* and at their proper meaning, and a gate that also refused
//! those would be a gate people work around — an exception list grows, and a
//! rule with exceptions stops being read. So the sweep forbids two words and
//! `the_narrowness_is_the_point` plants all four of the legitimate ones and
//! asserts they pass.
//!
//! **What is scanned.** Every directory at or below `templates/` that carries a
//! `config.json` — the substrate's own definition of "there is a cell here" —
//! minus `seed/` subtrees, which are DATA and never an address (`parse_subtree`
//! refuses to descend into one for exactly that reason, so a `config.json` down
//! there is a row's payload and its directory name addresses nothing).

/// The repository root, two levels above this crate.
fn core_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The ruled deny-list: the forbidden word, and the name Q19 ruled in its
/// place. The replacement travels with the word so that a finding tells the
/// author what to type instead of only what not to.
const GLOSSARY_COLLISIONS: &[(&str, &str)] = &[("registry", "catalog"), ("scheduler", "calendar")];

/// Every directory at or below `root` that carries a `config.json`, sorted.
///
/// `seed/` and everything under it is skipped: a seed subtree is data, not
/// cells, and the substrate's own walk never descends into one — a directory
/// name down there is never an address, so refusing it would be a finding about
/// nothing.
fn cell_directories(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    collect(root, &mut out);
    out.sort();
    out
}

fn collect(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    if dir.file_name().map(|n| n == "seed").unwrap_or(false) {
        return;
    }
    if dir.join("config.json").is_file() {
        out.push(dir.to_path_buf());
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            collect(&p, out);
        }
    }
}

/// One sentence per colliding directory. `checked` counts the directories the
/// sweep actually read, so "nothing wrong" can be told apart from "nothing
/// looked at".
fn findings(templates_root: &std::path::Path, checked: &mut usize) -> Vec<String> {
    let mut out = Vec::new();
    for dir in cell_directories(templates_root) {
        *checked += 1;
        let Some(name) = dir.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };
        let Some((_, replacement)) = GLOSSARY_COLLISIONS.iter().find(|(word, _)| *word == name)
        else {
            continue;
        };
        let shown = dir
            .strip_prefix(templates_root)
            .unwrap_or(&dir)
            .display()
            .to_string();
        out.push(format!(
            "templates/{shown}: the directory name {name:?} is a substrate glossary word, so every \
             instance grown from this template carries a node addressed {name:?} beside the \
             substrate's own meaning of it. Q19 (GH #26) ruled the replacement: name it \
             {replacement:?}. A name is cheap to change now and expensive after the first instance \
             is grown."
        ));
    }
    out
}

#[test]
fn no_shipped_template_directory_takes_a_name_from_the_substrate_glossary() {
    let root = core_root().join("templates");
    let mut checked = 0usize;
    let found = findings(&root, &mut checked);
    assert!(
        found.is_empty(),
        "app-hive names collide with the substrate glossary:\n  {}",
        found.join("\n  ")
    );
    // A sweep that walked past the tree is the same green as a clean tree from
    // the outside, and that is how a directory gate quietly stops working.
    assert!(
        checked > 1,
        "the sweep read {checked} directories under {} — wrong root, or the walk broke",
        root.display()
    );
}

// ─────────────────────────────────────────────────── the tests of the test

/// Build a throwaway templates root from directory paths, each given an empty
/// `config.json` so that the walk counts it as a cell.
fn planted(dirs: &[&str]) -> tempfile::TempDir {
    let td = tempfile::TempDir::new().unwrap();
    for dir in dirs {
        let d = td.path().join(dir);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("config.json"), "{}").unwrap();
    }
    td
}

/// Both ruled words bite, and each finding names the path and the replacement —
/// a finding that only said "bad name" would leave the author to guess which
/// name Q19 ruled.
#[test]
fn both_ruled_words_are_refused_and_the_finding_names_the_replacement() {
    let td = planted(&["assistant/registry", "member/calendar-things/scheduler"]);
    let mut checked = 0usize;
    let found = findings(td.path(), &mut checked);
    assert_eq!(found.len(), 2, "expected exactly two findings: {found:#?}");
    let joined = found.join("\n");
    assert!(
        joined.contains("assistant/registry") && joined.contains("\"catalog\""),
        "the `registry` finding does not name its path and its replacement:\n{joined}"
    );
    assert!(
        joined.contains("calendar-things/scheduler") && joined.contains("\"calendar\""),
        "the `scheduler` finding does not name its path and its replacement:\n{joined}"
    );
}

/// The gate is two words wide on purpose. These four are cell-type and role
/// words the shipped library uses at their proper meaning; a gate that refused
/// them would be one people route around.
#[test]
fn the_narrowness_is_the_point() {
    let td = planted(&[
        "memory-hive/store",
        "talky/collector",
        "talky/dispatcher",
        "argus/timer",
    ]);
    let mut checked = 0usize;
    let found = findings(td.path(), &mut checked);
    assert!(
        found.is_empty(),
        "the sweep refused a legitimate role word: {found:#?}"
    );
    assert_eq!(checked, 4, "the sweep did not read all four planted cells");
}

/// A `seed/` subtree is data. A `config.json` down there is a row's payload and
/// its directory name is not an address, so it is not a naming defect.
#[test]
fn a_seed_subtree_is_data_and_not_an_address() {
    let td = planted(&["librarian/store/seed/registry"]);
    let mut checked = 0usize;
    let found = findings(td.path(), &mut checked);
    assert!(
        found.is_empty(),
        "a seed fixture was reported as an app-hive name: {found:#?}"
    );
    assert_eq!(
        checked, 0,
        "the sweep descended into a seed subtree it must not read"
    );
}
