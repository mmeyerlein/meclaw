//! GH #235 — the library table in `templates/README.md` is the door sign of the
//! app store, and it has to name what ships.
//!
//! `canvy` was exported with the public tree and had no row, which came to light
//! only because three numbers were compared by hand: the roadmap claimed
//! fourteen templates, the table had sixteen rows, and the export shipped
//! seventeen directories. Two of the three were wrong, and nothing was reading
//! any of them.
//!
//! **Every row names a template that exists, at the version it exists at.** A
//! row whose version has been superseded is worse than no row: it is an exact
//! reference (`README § Versioning`) that no longer resolves.
//!
//! The other half of the question -- every publicly exported template HAS a row
//! -- lives in `gh235_every_public_template_has_a_row.rs` and stays private,
//! because it reads the export allow-list out of `plans/`. That directory has no
//! public subset, so a reference to it in an exported test is dead by
//! construction; skipping instead would ship a test that asserts nothing, which
//! is the defect GH #234 was about. Splitting is what lets THIS half travel.

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// The rows of the library table, as `(name, version)`. A row is a table line
/// whose first cell is a markdown link into a sibling directory — which is what
/// makes it a row about a template rather than any other table in the file.
fn table_rows() -> Vec<(String, String)> {
    let raw = std::fs::read_to_string(repo("templates/README.md")).expect("templates/README.md");
    let mut out = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if !line.starts_with('|') {
            continue;
        }
        let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
        if cells.len() < 3 {
            continue;
        }
        // `[`name`](name/)`
        let Some(rest) = cells[0].strip_prefix("[`") else {
            continue;
        };
        let Some((name, _)) = rest.split_once("`](") else {
            continue;
        };
        out.push((name.to_string(), cells[1].to_string()));
    }
    out
}

#[test]
fn every_row_names_a_template_at_the_version_it_ships() {
    let rows = table_rows();
    assert!(
        rows.len() >= 16,
        "the library table parsed almost nothing: {rows:?}"
    );
    for (name, version) in &rows {
        let manifest = repo(&format!("templates/{name}/template.json"));
        let raw = std::fs::read_to_string(&manifest)
            .unwrap_or_else(|e| panic!("the table names `{name}`, which does not ship: {e}"));
        let val: meclaw_core::serde_json::Value =
            meclaw_core::serde_json::from_str(&raw).expect("template.json parses");
        assert_eq!(
            val["version"]
                .as_str()
                .expect("a template.json has a version"),
            version,
            "the table lists `{name}` at {version}; the shipped template is at \
             {} — a version in the table is an exact reference, so a stale one \
             does not resolve",
            val["version"]
        );
    }
}
