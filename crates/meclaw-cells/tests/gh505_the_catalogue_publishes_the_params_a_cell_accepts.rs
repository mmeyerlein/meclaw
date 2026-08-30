//! GH #505 — the catalogue names templates and never says what may be set on them.
//!
//! A catalogue row leads with the demands an operation owes: `CONTRACT —` is
//! what an instantiation owes (GH #292/#347), `STORES —` is what a `seed_rows`
//! owes (GH #483). `override_params` is the third demand of the same shape,
//! enforced by the same door
//! (`crates/meclaw-colony/src/mutation/subtree.rs::check_override_params`,
//! which reads the addressed cell's `params` key set straight out of its
//! `config.json`) — and it was unpublished.
//!
//! Measured on a builder stress run that wanted a periodic feed. The lane named
//! `clock@1.0.0` correctly, read the row, and found a `CONTRACT —` line, a
//! `STORES —` line and the sentence *"the cadence is `override_params` on the
//! node per instance"* — with no param name anywhere in it. Three rounds, three
//! refusals: `override_params['interval_ms']` → *"names no param of timer in
//! template 'clock'. Its params are: 'query_timeout_ms', 'schedules'"*, then
//! `override_params['cron']` → the same refusal, then a `schedules` entry the
//! timer refused for a missing `emit_to`. The door prints the list the corpus
//! does not carry.
//!
//! For the four blank single-cell templates of GH #482 — `clock`, `fetcher`,
//! `scriptlet`, `shelf` — that list is the WHOLE interface: they exist in order
//! to be overridden (`scriptlet`'s logic is `script_inline`, `shelf`'s table is
//! `schema`), so a catalogue that does not name their params does not describe
//! them at all.

use meclaw_core::serde_json::Value;
use std::path::{Path, PathBuf};

/// The retriever's own truncation (`builder-librarian/retrieve`). Mirrored, not
/// imported — a demand only legible past the cut is not published.
const RETRIEVED_CHARS: usize = 1200;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../")
}

fn corpus() -> Option<Vec<Value>> {
    let p = repo_root().join("templates/builder-librarian/store/seed/docs.jsonl");
    let raw = std::fs::read_to_string(p).ok()?;
    Some(
        raw.lines()
            .filter_map(|l| meclaw_core::serde_json::from_str::<Value>(l).ok())
            .filter(|v| v.get("id").is_some())
            .collect(),
    )
}

fn catalogue_row<'a>(rows: &'a [Value], name: &str) -> &'a Value {
    rows.iter()
        .find(|r| r["kind"].as_str() == Some("template") && r["section"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("no catalogue row for `{name}`"))
}

/// The template names in the library, sorted.
fn template_names() -> Vec<String> {
    let root = repo_root().join("templates");
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join("template.json").is_file())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    names
}

/// `(relative path, cell type, param keys)` for every `config.json` of one
/// template's OWN tree, on the walk `parse_subtree` makes: a `seed/` directory
/// is not a cell and is not descended into, and nothing below a `ref` is read.
fn own_cells(dir: &Path, base: &Path, out: &mut Vec<(String, String, Vec<String>)>) {
    let cfg_path = dir.join("config.json");
    let named_seed = dir.file_name().and_then(|n| n.to_str()) == Some("seed");
    if named_seed {
        return;
    }
    if cfg_path.is_file() {
        let raw = std::fs::read_to_string(&cfg_path).unwrap_or_default();
        let cfg: Value = meclaw_core::serde_json::from_str(&raw).unwrap_or(Value::Null);
        let ty = cfg["cell"]["type"].as_str().unwrap_or("").to_string();
        let rel = dir
            .strip_prefix(base)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        let mut keys: Vec<String> = cfg["params"]
            .as_object()
            .map(|o| o.keys().cloned().collect())
            .unwrap_or_default();
        keys.sort();
        out.push((rel, ty.clone(), keys));
        if ty == "ref" {
            return;
        }
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    for d in dirs {
        own_cells(&d, base, out);
    }
}

fn cells_of(name: &str) -> Vec<(String, String, Vec<String>)> {
    let base = repo_root().join("templates").join(name);
    let mut out = Vec::new();
    own_cells(&base, &base, &mut out);
    out.sort();
    out
}

/// Where the `PARAMS —` line begins, or a panic naming what the row does say.
fn params_offset(text: &str, name: &str) -> usize {
    text.find("PARAMS —").unwrap_or_else(|| {
        panic!(
            "the catalogue row of `{name}` carries no `PARAMS —` line, so a \
             composer that looked the template up is told it may be overridden \
             and never what may stand in the override: {}",
            &text[..text.len().min(400)]
        )
    })
}

#[test]
fn the_four_blank_templates_publish_every_param_they_accept() {
    let Some(rows) = corpus() else {
        return; // R2b: this tree does not carry the corpus.
    };
    for name in ["clock", "fetcher", "scriptlet", "shelf"] {
        let cells = cells_of(name);
        assert_eq!(
            cells.len(),
            1,
            "`{name}` is meant to be the single-cell template of its type"
        );
        let (_, ty, keys) = &cells[0];
        assert!(
            !keys.is_empty(),
            "`{name}` declares no params -- this gate would pass vacuously"
        );
        let text = catalogue_row(&rows, name)["text"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let at = params_offset(&text, name);
        assert!(
            at < RETRIEVED_CHARS,
            "`{name}`: the params line begins at character {at}, past the \
             retriever's cut -- a demand only legible past the cut is not published"
        );
        let line = text[at..].lines().next().unwrap_or("");
        assert!(
            line.contains("FLAT"),
            "`{name}` is ONE cell, so `override_params` is a flat params object \
             and the line has to say so: {line}"
        );
        assert!(
            line.contains(ty.as_str()),
            "`{name}`: the line does not name the cell type `{ty}` the refusal \
             names: {line}"
        );
        for k in keys {
            assert!(
                line.contains(k.as_str()),
                "`{name}`: the params line does not name `{k}`, and the door \
                 refuses every key that is not in that set -- with nothing of \
                 the manifest applied: {line}"
            );
        }
    }
}

#[test]
fn every_catalogue_row_says_what_may_be_overridden() {
    let Some(rows) = corpus() else {
        return;
    };
    let names = template_names();
    assert!(!names.is_empty(), "no templates found -- vacuous gate");
    for name in names {
        let text = catalogue_row(&rows, &name)["text"].as_str().unwrap_or("");
        let at = params_offset(text, &name);
        let line = text[at..].lines().next().unwrap_or("");
        // Ends inside the window, not merely starts inside it: a cell list whose
        // tail falls past the cut reads as the complete set and is not one --
        // the silent-truncation shape GH #344 measured one level up.
        assert!(
            at + line.chars().count() <= RETRIEVED_CHARS,
            "`{name}`: the params line runs to character {}, past the \
             retriever's cut of {RETRIEVED_CHARS} -- published half, and it \
             looks whole",
            at + line.chars().count()
        );
    }
}

#[test]
fn a_cut_params_line_carries_the_rest_in_rows_of_its_own() {
    // The cap moves content, it does not delete it. Whatever the catalogue line
    // could not hold is published as `params` rows, and between them they name
    // every cell and every param of the template.
    let Some(rows) = corpus() else {
        return;
    };
    let mut cut = 0;
    for name in template_names() {
        let text = catalogue_row(&rows, &name)["text"].as_str().unwrap_or("");
        let at = params_offset(text, &name);
        if !text[at..]
            .lines()
            .next()
            .unwrap_or("")
            .contains("more cell(s)")
        {
            continue;
        }
        cut += 1;
        let pages: String = rows
            .iter()
            .filter(|r| {
                r["kind"].as_str() == Some("params")
                    && r["section"].as_str().is_some_and(|s| {
                        s == format!("{name} params") || s.starts_with(&format!("{name} params ("))
                    })
            })
            .filter_map(|r| r["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !pages.is_empty(),
            "`{name}`: the catalogue line says it cut the list and no `params` \
             row carries the rest -- which is the silent delete again"
        );
        for (rel, _ty, keys) in cells_of(&name) {
            let addressed = if rel.is_empty() {
                "\"\"".to_string()
            } else {
                rel.clone()
            };
            assert!(
                pages.contains(&addressed),
                "`{name}`: no `params` row names the cell `{addressed}`"
            );
            for k in keys {
                assert!(
                    pages.contains(k.as_str()),
                    "`{name}`/{addressed}: no `params` row names `{k}`"
                );
            }
        }
    }
    assert!(cut > 0, "no template's list was cut -- vacuous gate");
}

#[test]
fn a_subtree_row_addresses_its_cells_by_path() {
    // The addressed form is the one a composite takes, and the address is the
    // cell's path inside the template (`""` being the root). A row that named
    // the keys without the paths would be unusable on anything but a
    // single-cell template.
    let Some(rows) = corpus() else {
        return;
    };
    let cells = cells_of("submit");
    assert!(
        cells.len() > 1,
        "`submit` stopped being a subtree -- pick another witness"
    );
    let text = catalogue_row(&rows, "submit")["text"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let at = params_offset(&text, "submit");
    assert!(at < RETRIEVED_CHARS, "the params line is past the cut");
    let line = text[at..].lines().next().unwrap_or("");
    for (rel, _ty, keys) in &cells {
        let addressed = if rel.is_empty() { "\"\"" } else { rel.as_str() };
        assert!(
            line.contains(addressed),
            "`submit`: the params line does not address the cell `{addressed}`: {line}"
        );
        for k in keys {
            assert!(
                line.contains(k.as_str()),
                "`submit`/{addressed}: the params line does not name `{k}`: {line}"
            );
        }
    }
}

/// A drift lock in the sense of `docs/development-rules.md` § 2d: it greps the
/// promise on the public template surface AND drives the mechanism, because
/// either half alone lets the two walk apart.
#[test]
fn the_template_surface_promises_the_params_line_and_the_corpus_carries_it() {
    let descriptor = repo_root().join("templates/builder-librarian/template.json");
    let raw = std::fs::read_to_string(&descriptor).expect("the descriptor");
    let blob: Value = meclaw_core::serde_json::from_str(&raw).expect("parses");
    let purpose = blob["description"]["purpose"].as_str().unwrap_or("");
    assert!(
        purpose.contains("override_params"),
        "the descriptor stopped promising the params line -- then this lock is \
         guarding nothing: {purpose}"
    );

    let library = repo_root().join("templates/README.md");
    let table = std::fs::read_to_string(&library).expect("the library table");
    assert!(
        table.contains("`PARAMS --`"),
        "the library table no longer names the third demand line"
    );

    // And the mechanism the sentences describe: an arbitrary row of the shipped
    // corpus carries it, ahead of the retriever's cut.
    let Some(rows) = corpus() else {
        return;
    };
    let text = catalogue_row(&rows, "scriptlet")["text"]
        .as_str()
        .unwrap_or("");
    let at = params_offset(text, "scriptlet");
    assert!(at < RETRIEVED_CHARS, "the promise is past the cut");
    assert!(
        text[at..]
            .lines()
            .next()
            .unwrap_or("")
            .contains("script_inline"),
        "`scriptlet` exists to have its `script_inline` overridden, and the row \
         does not name it"
    );
}
