//! GH #486 — the design lane was told a level's edges are fixed, forbidden to
//! approximate them, and given no source that holds them.
//!
//! The briefing's `LEVELS` block publishes the COUNTS and names the renderer
//! that holds the SETS — and that renderer is on the fast lane, unreachable
//! from the design lane. The question is then whether the corpus holds them,
//! and it did not: eighty-five distinct sources, and not one match for
//! `grow-`. `librarian_search` cannot return what was never indexed, and
//! `catalogue_lookup` returns a `template.json`, which describes a template and
//! not the edges a parent draws around one.
//!
//! Measured: two wishes that grow levels with something extra in them — the
//! exact class the block says reaches the design lane — spent all seven rounds
//! reconstructing `grow-screen.json` from prose, one guard at a time, and ended
//! without a manifest. Telling the model its budget in the wish itself changed
//! nothing, which is the useful half of that measurement: it rules out "the
//! model does not know it should stop" as the whole story and leaves "the model
//! cannot find what it was told to be exact about".
//!
//! `examples/organism/grow-*.json` are the rendered sets and they are already
//! byte-pinned against the renderer by
//! `gh466_grow_level_renders_the_level.rs`. Indexing them makes a correct
//! artefact do a second job.
//!
//! **A chunk that does not survive retrieval is not indexed.**
//! `builder-librarian/retrieve` cuts every row it returns to the window its
//! KIND is given, so a level whose set is published in a 2900-character chunk
//! is published as its first two edges. That is why this file asserts the size
//! as well as the content: the retrieval unit is the unit that has to be
//! complete. Since GH #543 a level row has a window of its own, for exactly
//! that reason.

use meclaw_core::serde_json::Value;
use std::collections::BTreeSet;
use std::path::PathBuf;

/// The retriever's own truncation for a LEVEL row. Mirrored, not imported — the
/// point of the gate is to be red if the two ever disagree about what arrives
/// whole.
///
/// It is the THIRD window `builder-librarian/retrieve` keeps, and it exists
/// because of what this file asserts: a level row had lived inside the ordinary
/// 1200 with fourteen characters to spare, which is a coincidence rather than a
/// margin, and GH #543 spent it. The set is what must survive, so the window
/// moved rather than the set (`params.level_chars` of `./retrieve`).
const RETRIEVED_CHARS: usize = 1600;

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

/// Every `hop.route` literal a condition names, plus the edge count.
fn lanes_of(decl: &Value) -> (BTreeSet<String>, usize) {
    let mut lanes = BTreeSet::new();
    let edges = decl["diff"]["add_edges"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for e in &edges {
        let c = e["condition"].as_str().unwrap_or("");
        let needle = "hop.route == '";
        let mut from = 0;
        while let Some(i) = c[from..].find(needle) {
            let start = from + i + needle.len();
            let end = start + c[start..].find('\'').unwrap_or(0);
            if end > start {
                lanes.insert(c[start..end].to_string());
            }
            from = end.max(start + 1);
        }
    }
    (lanes, edges.len())
}

/// The rendered level sets that ship, as (relative source path, declaration).
fn grown_levels() -> Vec<(String, Value)> {
    let dir = repo_root().join("examples/organism");
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return out;
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with("grow-") && n.ends_with(".json"))
        .collect();
    names.sort();
    for n in names {
        let Ok(raw) = std::fs::read_to_string(dir.join(&n)) else {
            continue;
        };
        let Ok(decl) = meclaw_core::serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        // One file is one SOURCE, and the corpus row is per source. Since GH
        // #503 a file may carry more than one declaration — a screen and an app
        // declare themselves at two different containers and cannot share a
        // mutation any more — so what is published is the union of the file's
        // edges, exactly as before the split.
        let decl = match decl["manifest"].as_array() {
            Some(list) => {
                let edges: Vec<Value> = list
                    .iter()
                    .flat_map(|d| {
                        d["diff"]["add_edges"]
                            .as_array()
                            .cloned()
                            .unwrap_or_default()
                    })
                    .collect();
                meclaw_core::serde_json::json!({"diff": {"add_edges": edges}})
            }
            None => decl,
        };
        // `grow-os.json` grows the shell, not a level: no edges, nothing to
        // publish.
        if decl["diff"]["add_edges"]
            .as_array()
            .is_none_or(|a| a.is_empty())
        {
            continue;
        }
        out.push((format!("examples/organism/{n}"), decl));
    }
    out
}

#[test]
fn every_rendered_level_set_is_a_corpus_row() {
    let Some(rows) = corpus() else {
        return; // R2b: this tree does not carry the corpus.
    };
    let levels = grown_levels();
    assert!(
        !levels.is_empty(),
        "no rendered level sets found -- this gate would pass vacuously"
    );
    for (source, decl) in levels {
        let mine: Vec<&Value> = rows
            .iter()
            .filter(|r| r["source"].as_str() == Some(source.as_str()))
            .collect();
        assert!(
            !mine.is_empty(),
            "{source} is in no chunk of the corpus: the design lane is asked to \
             reproduce this set exactly and has no way to look it up"
        );
        let (lanes, count) = lanes_of(&decl);
        let all: String = mine
            .iter()
            .map(|r| r["text"].as_str().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");
        for lane in &lanes {
            assert!(
                all.contains(lane.as_str()),
                "{source}: the corpus row does not name the `{lane}` lane, so a \
                 composer reading it would draw a subset -- a level wired with \
                 nine of its fourteen edges boots and answers nothing"
            );
        }
        assert!(
            all.contains(&count.to_string()),
            "{source}: the corpus row does not say how many edges the set has \
             ({count}), and the count is the one thing a reader can check its \
             own draft against"
        );
    }
}

#[test]
fn a_level_row_survives_the_retriever_whole() {
    let Some(rows) = corpus() else {
        return;
    };
    for (source, _) in grown_levels() {
        for r in rows
            .iter()
            .filter(|r| r["source"].as_str() == Some(source.as_str()))
        {
            let text = r["text"].as_str().unwrap_or("");
            assert!(
                text.chars().count() <= RETRIEVED_CHARS,
                "{source}: chunk {} is {} characters, and the retriever hands \
                 the model the first {RETRIEVED_CHARS} -- a fixed set published \
                 half is a set that will be drawn half",
                r["id"],
                text.chars().count()
            );
        }
    }
}

#[test]
fn the_briefing_points_at_the_source_it_can_reach() {
    // The LEVELS block used to point at `grow_level`, the FAST LANE renderer:
    // the one thing on the other side of the lane the wish already failed to
    // take. "Be exact" is only a rule if the exact thing is reachable.
    let p = repo_root().join("templates/builder/brief/config.json");
    let raw = std::fs::read_to_string(&p).expect("brief config");
    let cfg: Value = meclaw_core::serde_json::from_str(&raw).expect("parses");
    let script = cfg["params"]["script_inline"].as_str().expect("script");
    let levels = script
        .find("LEVELS --")
        .map(|i| script[i..].to_string())
        .expect("the LEVELS block");
    assert!(
        levels.contains("examples/organism/grow-"),
        "the LEVELS block does not name the source a composer can actually \
         retrieve the set from"
    );
    assert!(
        levels.contains("librarian_search"),
        "and it does not say which tool answers with it"
    );
}
