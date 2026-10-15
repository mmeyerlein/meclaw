//! GH #1209 -- the product does not know the tests' filter switch.
//!
//! Owner ruling (2026-10-12): in the tests the special filters are off; the test
//! driver switches them off (`workshop/tools/lib/no-filters.mjs`), the product
//! carries no test branch. Nothing under `templates/` or a crate's `src/` names the
//! switch, and the sheet still draws its filters.
//!
//! A static read of the tree, no browser: it lives apart from the stages of
//! `gh1209_no_stage_draws_a_filter_browser.rs`, because it reads every crate's
//! `src/` -- in a browser unit that read reached the browser station from every Rust
//! diff (`scripts/gate_plan.py` rule 8).
//!
//! Without the template library the test says `SKIP` and passes only in the public
//! export (R2b), recognised by the missing root `plans/`, the directory that never
//! travels (the marker `gh80_shipped_conditions_are_guarded` uses, as gh203/gh212 do:
//! a forbidden prefix cannot be promoted onto the export's allow-list). A private tree
//! without the library is red: there the skip would hide the scan (re-check N GN1, H2).

use std::path::{Path, PathBuf};

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const SHEET: &str = "templates/display/compose/display-dna.css";

/// The words of the switch; none of them belongs in the product.
const SWITCH_WORDS: &[&str] = &[
    "no-filters",
    "NO_FILTERS",
    "noFilters",
    "withoutFilters",
    "adoptedStyleSheets",
    "webdriver",
];

/// The export marker, probed and never read (`gh80_shipped_conditions_are_guarded`):
/// `plans/` is an export-forbidden prefix, so only the private tree has it.
const PRIVATE_ONLY_MARKER: &str = "plans";

/// Whether the tree at `root` carries the template library this scan reads.
fn library_ships(root: &Path) -> bool {
    root.join("templates/display/template.json").is_file()
}

/// Whether the scan runs on the tree at `root`: yes where the library ships; a quiet
/// skip only in the public export (no `plans/`).
fn the_scan_runs(root: &Path) -> bool {
    if library_ships(root) {
        return true;
    }
    assert!(
        !root.join(PRIVATE_ONLY_MARKER).is_dir(),
        "the private tree ({}) does not carry templates/display/template.json: the scan for \
         the test's filter switch cannot run, and a skip here would read green",
        root.display()
    );
    println!("SKIP the template library does not ship in the public export");
    false
}

/// Every file under `dir` whose name ends in one of `exts`, `node_modules` aside.
fn files(dir: &Path, exts: &[&str], out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            if name != "node_modules" && !name.starts_with('.') {
                files(&path, exts, out);
            }
        } else if exts.iter().any(|e| name.ends_with(e)) && !name.starts_with(".env") {
            out.push(path);
        }
    }
}

#[test]
fn the_product_carries_no_test_switch() {
    if !the_scan_runs(&repo(".")) {
        return;
    }
    let mut all = Vec::new();
    files(
        &repo("templates"),
        &[
            ".css", ".js", ".mjs", ".py", ".json", ".html", ".heex", ".ex",
        ],
        &mut all,
    );
    let crates = repo("crates");
    for entry in std::fs::read_dir(&crates).expect("crates").flatten() {
        files(&entry.path().join("src"), &[".rs"], &mut all);
    }
    assert!(
        all.len() > 100,
        "the product was found: {} files",
        all.len()
    );
    for path in &all {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        for word in SWITCH_WORDS {
            assert!(
                !text.contains(word),
                "{}: the product names the test's filter switch (`{word}`)",
                path.display()
            );
        }
    }
    // The sheet keeps its glass: the switch lives in the drivers, not here.
    let sheet = std::fs::read_to_string(repo(SHEET)).expect("the sheet reads");
    assert!(
        sheet.contains("backdrop-filter:") && sheet.contains("filter: blur("),
        "the sheet still draws its filters; switching them off is the test driver's job"
    );
    for gate in [
        "[data-test",
        "[data-filters",
        "[data-no-filter",
        ".no-filters",
    ] {
        assert!(
            !sheet.contains(gate),
            "the sheet gates a rule on a test switch (`{gate}`)"
        );
    }
}

/// The guard bites (re-check N GN1, H2): a tree without the library skips only when it
/// is the public export; with `plans/` beside the crates the missing library is red.
#[test]
fn a_private_tree_without_the_library_is_red() {
    let public = tempfile::tempdir().expect("a scratch tree");
    assert!(!the_scan_runs(public.path()), "the public export skips");

    let private = tempfile::tempdir().expect("a scratch tree");
    std::fs::create_dir(private.path().join(PRIVATE_ONLY_MARKER))
        .expect("the private-only marker directory");
    let verdict = std::panic::catch_unwind(|| the_scan_runs(private.path()));
    assert!(
        verdict.is_err(),
        "a private tree without the template library skipped the scan: {verdict:?}"
    );

    let shipped = tempfile::tempdir().expect("a scratch tree");
    let display = shipped.path().join("templates/display");
    std::fs::create_dir_all(&display).expect("templates/display");
    std::fs::write(display.join("template.json"), "{}").expect("template.json");
    assert!(
        the_scan_runs(shipped.path()),
        "a tree with the library scans"
    );
}
