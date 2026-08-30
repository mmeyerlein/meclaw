//! GH #391 (§ 2d drift lock) — the countable claims on `talky`'s public
//! surface are derived from the shipped template tree, not from the diff that
//! last changed it.
//!
//! W5.7 added the sidecar `splitter` (GH #379) and three shipped documents kept
//! describing the composite as it stood before: one of them carried a cell
//! count that had been true two waves earlier. No test was red, because no test
//! ever read the sentence — the exact failure mode
//! `docs/development-rules.md` § 2d exists for.
//!
//! Both halves, per § 2d: the sentences are read out of the shipped README
//! (prose), and every number in them is derived from `templates/talky/` and the
//! templates it references (mechanism). Adding a cell or an edge without moving
//! the prose is red here; so is rewording the prose away from the mechanism.
//!
//! A `ref` cell is expanded, because a reader counting cells in a running
//! colony counts what the registry holds, and the registry holds the referenced
//! sub-units (GH #277) — `talky` names three of them and carries copies of none.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn repo_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// The shipped template, or `None` in a tree that does not carry it. The public
/// clone does carry `templates/`, so this normally resolves; the guard is here
/// so the test skips instead of failing where the library was not exported.
fn shipped_talky() -> Option<PathBuf> {
    let root = repo_path("templates/talky");
    root.join("config.json").exists().then_some(root)
}

/// Every `config.json` under `dir`, in path order.
fn config_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            out.extend(config_files(&p));
        } else if p.file_name().is_some_and(|n| n == "config.json") {
            out.push(p);
        }
    }
    out
}

fn read_json(p: &Path) -> serde_json::Value {
    let text = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{} reads: {e}", p.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{} parses: {e}", p.display()))
}

/// What one template directory contributes, with `ref` cells expanded into the
/// template they name. Returns (cells, hive markers).
fn count_nodes(dir: &Path) -> (usize, usize) {
    let (mut cells, mut hives) = (0usize, 0usize);
    for p in config_files(dir) {
        let v = read_json(&p);
        match v["cell"]["type"].as_str() {
            Some("hive") => hives += 1,
            Some("ref") => {
                // `collector@3.0.0` — the registry resolves the name, this test
                // resolves the directory beside it.
                let target = v["cell"]["template"]
                    .as_str()
                    .unwrap_or_else(|| panic!("{} is a ref without a template", p.display()));
                let name = target.split('@').next().unwrap_or(target);
                let (c, h) = count_nodes(&repo_path(&format!("templates/{name}")));
                cells += c;
                hives += h;
            }
            Some(_) => cells += 1,
            None => panic!("{} declares no cell type", p.display()),
        }
    }
    (cells, hives)
}

/// The edges `talky`'s own `params.graph` draws, split the way the README
/// splits them: an edge with `.` at either end is the hive boundary, everything
/// else is a round inside it.
fn count_edges(root: &Path) -> (usize, usize, usize, usize) {
    let v = read_json(&root.join("config.json"));
    let edges = v["params"]["graph"]["edges"]
        .as_array()
        .expect("talky draws a graph");
    let mut internal = 0;
    let mut from_door = 0;
    let mut to_door = 0;
    for e in edges {
        let from = e["from"].as_str().unwrap_or_default();
        let to = e["to"].as_str().unwrap_or_default();
        match (from, to) {
            (".", _) => from_door += 1,
            (_, ".") => to_door += 1,
            _ => internal += 1,
        }
    }
    (edges.len(), internal, from_door, to_door)
}

/// The number words the README writes. Small on purpose: a count that outgrows
/// this table is a count the prose should stop spelling out.
fn word(n: usize) -> &'static str {
    let table: BTreeMap<usize, &'static str> = [
        (3, "three"),
        (4, "four"),
        (5, "five"),
        (8, "eight"),
        (9, "nine"),
        (10, "ten"),
        (11, "eleven"),
        (12, "twelve"),
        (13, "thirteen"),
        (14, "fourteen"),
        (15, "fifteen"),
        (16, "sixteen"),
        (24, "twenty-four"),
        (26, "twenty-six"),
        (27, "twenty-seven"),
        (28, "twenty-eight"),
        (31, "thirty-one"),
    ]
    .into_iter()
    .collect();
    table
        .get(&n)
        .unwrap_or_else(|| panic!("no word for {n} — extend the table with the prose"))
}

/// The one paragraph of the README that carries `needle`, joined into a single
/// line for the failure text.
///
/// Paragraphs, not lines: the README is hard-wrapped, so a sentence that names
/// three numbers routinely straddles a line break. Matching per line would make
/// the lock depend on where the wrapper happened to break, which is the one
/// property a drift lock must not have.
fn sentence_with(readme: &str, needle: &str) -> String {
    readme
        .split("\n\n")
        .map(|p| p.split_whitespace().collect::<Vec<_>>().join(" "))
        .find(|p| p.contains(needle))
        .unwrap_or_else(|| {
            panic!(
                "no paragraph of templates/talky/README.md carries `{needle}` — \
                 the prose this drift lock reads was reworded or removed. Move \
                 the lock with it (development-rules § 2d): a lock that silently \
                 stops finding its sentence pins nothing."
            )
        })
}

#[test]
fn the_readme_edge_counts_are_the_edges_talky_draws() {
    let Some(root) = shipped_talky() else {
        return;
    };
    let readme = std::fs::read_to_string(root.join("README.md")).expect("the README ships");
    let (total, internal, from_door, to_door) = count_edges(&root);
    let boundary = from_door + to_door;

    for (n, needle, what) in [
        (
            total,
            "edges, each of",
            "the total in the opening paragraph",
        ),
        (internal, "edges of round in this hive", "the rounds inside"),
    ] {
        let line = sentence_with(&readme, needle);
        assert!(
            line.to_lowercase().contains(word(n)),
            "{what}: templates/talky/README.md says something other than \
             `{}` ({n} measured from params.graph):\n  {line}",
            word(n)
        );
    }

    // The boundary sentence carries three numbers at once, and the two halves
    // have to add up to the whole — a split that drifts is the version of this
    // drift the totals alone cannot see.
    let line = sentence_with(&readme, "that ARE the boundary");
    for (n, what) in [
        (boundary, "the boundary total"),
        (from_door, "the door edges leaving `.`"),
        (to_door, "the edges arriving at `.`"),
    ] {
        assert!(
            line.to_lowercase().contains(word(n)),
            "{what}: the boundary sentence must say `{}` ({n} measured):\n  {line}",
            word(n)
        );
    }
    assert_eq!(
        internal + boundary,
        total,
        "the README splits the graph in two and the halves must be the whole"
    );
}

#[test]
fn the_readme_cell_count_is_the_cells_a_grown_talky_registers() {
    let Some(root) = shipped_talky() else {
        return;
    };
    let readme = std::fs::read_to_string(root.join("README.md")).expect("the README ships");
    let (cells, hives) = count_nodes(&root);

    let line = sentence_with(&readme, "hive markers)");
    assert!(
        line.to_lowercase().contains(word(cells)),
        "the boot recipe promises a cell count the tree does not have: {cells} \
         cells measured (a `ref` counts as the sub-units it names, GH #277), so \
         the sentence must say `{}`:\n  {line}",
        word(cells)
    );
    assert!(
        line.to_lowercase().contains(word(hives)),
        "the boot recipe names a hive-marker count the tree does not have \
         ({hives} measured, so `{}`):\n  {line}",
        word(hives)
    );
}
