//! GH #483 — `seed_rows` is told about a rule whose data it cannot reach.
//!
//! The briefing explains the constraint carefully and correctly: the table must
//! be one the target store's `params.schema` names (otherwise
//! `seed_table_undeclared`), and every key of every row must be a declared
//! column of that table. Both constraints are real. Neither the tables nor the
//! columns of any shipped store were in the corpus.
//!
//! Measured: a wish that needs exactly one `seed_rows` declaration spent all
//! seven rounds asking for the schema and never got it — seven model calls, a
//! prompt growing 5 528 → 32 281 tokens, and no manifest. Every guess in that
//! trace is a plausible column name and none of them is one of the real ones.
//! Counted over the corpus of the day: `window_minutes` **0** occurrences,
//! `quality_gate` **0**.
//!
//! This repeats the shape of the `CONTRACT —` line exactly, one operation
//! later. `requires.ctx` is the demand an INSTANTIATION owes and was
//! unpublished; a store's `params.schema` is the demand a `seed_rows` owes.
//!
//! The retrieval unit is the unit that has to be complete:
//! `builder-librarian/retrieve` hands the model `text[:1200]`, so a schema
//! published inside a 1 400-character chunk is published as two thirds of
//! itself. That is why the size is asserted beside the content — and why the
//! unit indexed is one TABLE, which is also the unit a `seed_rows` entry names.

use meclaw_core::serde_json::Value;
use std::path::PathBuf;

/// The retriever's own truncation. Mirrored, not imported.
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
/// Every shipped store, as (template, path inside it, the config.json that
/// declares it, table, columns).
fn shipped_stores() -> Vec<(String, String, String, String, Vec<String>)> {
    let root = repo_root().join("templates");
    let mut out = Vec::new();
    let Ok(templates) = std::fs::read_dir(&root) else {
        return out;
    };
    let mut names: Vec<String> = templates
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join("template.json").is_file())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    for name in names {
        let base = root.join(&name);
        let mut stack = vec![base.clone()];
        let mut found: Vec<(String, String, String, Vec<String>)> = Vec::new();
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in entries.filter_map(Result::ok) {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.file_name().and_then(|n| n.to_str()) == Some("config.json") {
                    let Ok(raw) = std::fs::read_to_string(&p) else {
                        continue;
                    };
                    let Ok(cfg) = meclaw_core::serde_json::from_str::<Value>(&raw) else {
                        continue;
                    };
                    if cfg["cell"]["type"] != "store" {
                        continue;
                    }
                    let rel = p
                        .parent()
                        .and_then(|d| d.strip_prefix(&base).ok())
                        .map(|d| d.to_string_lossy().to_string())
                        .unwrap_or_default();
                    // The source a corpus row must cite: the declaration itself, so
                    // the join between tree and corpus is a FILE and not a sentence
                    // the generator happens to write.
                    let source = format!(
                        "templates/{name}/{}",
                        p.strip_prefix(&base)
                            .map(|d| d.to_string_lossy().to_string())
                            .unwrap_or_default()
                    );
                    let Some(schema) = cfg["params"]["schema"].as_object() else {
                        continue;
                    };
                    for (table, cols) in schema {
                        let Some(cols) = cols.as_object() else {
                            continue;
                        };
                        found.push((
                            if rel.is_empty() {
                                ".".into()
                            } else {
                                rel.clone()
                            },
                            source.clone(),
                            table.clone(),
                            cols.keys().cloned().collect(),
                        ));
                    }
                }
            }
        }
        found.sort();
        for (path, source, table, cols) in found {
            out.push((name.clone(), path, source, table, cols));
        }
    }
    out
}

#[test]
fn every_shipped_store_publishes_its_tables_and_columns() {
    let Some(rows) = corpus() else {
        return; // R2b: this tree does not carry the corpus.
    };
    let stores = shipped_stores();
    assert!(
        !stores.is_empty(),
        "no shipped store found -- this gate would pass vacuously"
    );
    for (template, path, source, table, cols) in stores {
        // The row that publishes THIS table: it cites the declaration it was
        // derived from and names the table in backticks, so `firewall`'s store
        // at `./rules` with a table `rules` cannot be answered by its sibling.
        let quoted = format!("`{table}`");
        let hit = rows.iter().find(|r| {
            r["kind"].as_str() == Some("store")
                && r["source"].as_str() == Some(source.as_str())
                && r["text"].as_str().unwrap_or("").contains(&quoted)
        });
        let hit = hit.unwrap_or_else(|| {
            panic!(
                "no corpus row publishes `{template}` store `{path}` table \
                 `{table}` -- a composer is told `seed_rows` must satisfy a \
                 schema it cannot look up, which is what a whole build budget \
                 was measured being spent on"
            )
        });
        let text = hit["text"].as_str().unwrap_or("");
        for c in &cols {
            assert!(
                text.contains(c.as_str()),
                "{template}/{path}/{table}: the corpus row does not name the \
                 column `{c}`, and a row key that is not a declared column is \
                 refused -- with nothing of the manifest applied"
            );
        }
        assert!(
            text.chars().count() <= RETRIEVED_CHARS,
            "{template}/{path}/{table}: the row is {} characters and the \
             retriever hands over the first {RETRIEVED_CHARS} -- a schema \
             published half is a schema guessed at",
            text.chars().count()
        );
    }
}

#[test]
fn the_catalogue_row_says_which_stores_a_template_carries() {
    // The second contract line, beside `CONTRACT —`. `catalogue_lookup` is the
    // call the briefing asks for by name; a composer that has looked a template
    // up must learn there that it has a store at all.
    let Some(rows) = corpus() else {
        return;
    };
    let row = rows
        .iter()
        .find(|r| r["kind"].as_str() == Some("template") && r["section"].as_str() == Some("argus"))
        .expect("the argus catalogue row");
    let text = row["text"].as_str().unwrap_or("");
    assert!(
        text.starts_with("CONTRACT —"),
        "the contract line still leads: {}",
        &text[..text.len().min(80)]
    );
    assert!(
        text.contains("STORES —"),
        "the catalogue row names no stores, so a composer that looked the \
         template up learns it has none: {}",
        &text[..text.len().min(400)]
    );
    for needle in ["charter", "goals", "receipts"] {
        assert!(
            text.contains(needle),
            "the stores line does not name `{needle}`"
        );
    }
    let stores_at = text.find("STORES —").expect("the stores line");
    assert!(
        stores_at < RETRIEVED_CHARS,
        "the stores line begins at character {stores_at}, past the \
         retriever's truncation -- a contract that is only legible past the cut \
         is not published"
    );
}

#[test]
fn the_briefing_says_where_the_schema_is_published() {
    let p = repo_root().join("templates/builder/brief/config.json");
    let raw = std::fs::read_to_string(&p).expect("brief config");
    let cfg: Value = meclaw_core::serde_json::from_str(&raw).expect("parses");
    let script = cfg["params"]["script_inline"].as_str().expect("script");
    let rows_block = script
        .find("ROWS --")
        .map(|i| {
            let rest = &script[i..];
            rest[..rest.find("\\n\\n").unwrap_or(rest.len())].to_string()
        })
        .expect("the ROWS block");
    assert!(
        rows_block.contains("librarian_search") || rows_block.contains("catalogue_lookup"),
        "the ROWS block states a rule about `params.schema` and names no way to \
         read one: {rows_block}"
    );
}

/// The catalogue was a promise with no mechanism, which is how the composer
/// could not find a template it named exactly.
///
/// `templates/builder/lib` stamps `hop.lib_kind = "template"` for a
/// `catalogue_lookup`, and its own `emits_meaning` publishes that as *"the same
/// corpus filtered to the rows that open with `CONTRACT —`"*. Nothing read the
/// key: both tools ran the identical unfiltered BM25 query. Measured in the
/// design runs — `catalogue_lookup "member"` came back with `org` at position
/// one and `member` at position four, as a continuation chunk — which is a tool
/// a model cannot learn to use, so it asks again.
///
/// A drift lock in the sense of `docs/development-rules.md` § 2d: it greps the
/// sentence AND drives the mechanism, because either half alone lets the two
/// walk apart.
#[test]
fn catalogue_lookup_is_the_catalogue_and_not_the_corpus() {
    use meclaw_core::serde_json::json;
    use meclaw_testing::{emit_one, shipped_script};

    let lib = repo_root().join("templates/builder/lib/config.json");
    let promise: Value =
        meclaw_core::serde_json::from_str(&std::fs::read_to_string(&lib).expect("lib config"))
            .expect("parses");
    assert!(
        promise["description"]["emits_meaning"]
            .as_str()
            .unwrap_or("")
            .contains("filtered"),
        "the lib cell stopped promising a filtered catalogue -- then this lock \
         is guarding nothing"
    );

    let stamped = emit_one(
        &shipped_script(lib.to_str().expect("path")),
        &json!({
            "header": {"hop": {"tool_name": "catalogue_lookup", "tool_call_id": "c1"},
                       "context": {}},
            "params": {},
            "messages": [{"origin": "assistant", "type": "tool_call", "id": "c1",
                          "text": "{\"query\": \"member\"}"}],
        }),
    );
    assert_eq!(
        stamped["header"]["lib_kind"], "template",
        "the adapter no longer marks a catalogue lookup: {}",
        stamped["header"]
    );

    let retrieve = repo_root().join("templates/builder-librarian/retrieve/config.json");
    let asked = emit_one(
        &shipped_script(retrieve.to_str().expect("path")),
        &json!({
            "header": {"hop": {"route": "in_request", "lib_kind": "template"},
                       "context": {}},
            "params": {},
            "messages": [{"origin": "user", "type": "text", "id": "", "text": "member"}],
        }),
    );
    let op: Value =
        meclaw_core::serde_json::from_str(asked["messages"][0]["text"].as_str().unwrap_or("{}"))
            .expect("the store op");
    assert_eq!(
        op["where"]["kind"], "template",
        "the librarian runs the SAME unfiltered query for both tools, so \
         `catalogue_lookup` answers a question its schema does not name: {op}"
    );

    // And the unmarked call is still the whole corpus: `librarian_search` is
    // how a composer reaches the store schemas and the level sets this issue
    // put there.
    let plain = emit_one(
        &shipped_script(retrieve.to_str().expect("path")),
        &json!({
            "header": {"hop": {"route": "in_request"}, "context": {}},
            "params": {},
            "messages": [{"origin": "user", "type": "text", "id": "", "text": "member"}],
        }),
    );
    let op: Value =
        meclaw_core::serde_json::from_str(plain["messages"][0]["text"].as_str().unwrap_or("{}"))
            .expect("the store op");
    assert!(
        op.get("where").is_none(),
        "an unmarked search must stay unfiltered, or the level and store rows \
         become unreachable: {op}"
    );
}
