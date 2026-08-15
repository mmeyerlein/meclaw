//! 0.2.0 P3 (issue #14, ruling Q6): morphology in the keyword leg, pinned at
//! the level the memory lanes actually use — the `search` op, not a raw
//! `MATCH`.
//!
//! The live defect this package closes: a fact was keyword-invisible for
//! exactly the question it answers, because FTS5 prefix-matches only at the END
//! of a word and the question was asked in the plural. Only the recency leg
//! carried it, which is luck, not retrieval.

use meclaw_cells::store::StoreParams;
use meclaw_cells::store::ddl::{apply_fts_ddl, apply_schema_ddl};
use meclaw_cells::store::ops::dispatch;
use meclaw_core::serde_json::json;

/// The memory hive's `facts` shape, reduced to what the keyword leg reads: the
/// claim and the canonical predicate (the two columns `params.fts` names since
/// 0.2.0 P2).
fn facts_store(rows: &[(&str, &str, &str)]) -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    // Exactly what the factory installs before any DDL runs.
    meclaw_cells::store::query::install_connection_extensions(&conn).unwrap();
    let params = StoreParams::parse(&json!({
        "schema": {"facts": {"id":"text","predicate":"text",
                             "canonical_predicate":"text","claim":"text"}},
        "fts": {"facts": ["claim", "canonical_predicate"]}
    }))
    .unwrap();
    apply_schema_ddl(&conn, &params.schema).unwrap();
    apply_fts_ddl(&conn, &params.fts, &params.canonical).unwrap();
    for (id, predicate, claim) in rows {
        let out = dispatch(
            &conn,
            &json!({"operation":"insert","table":"facts",
                    "row":{"id":id,"predicate":predicate,
                           "canonical_predicate":predicate,"claim":claim}}),
        )
        .unwrap();
        assert_eq!(out.rows_affected, 1);
    }
    conn
}

/// One recall-lane query: the tokens quoted and prefix-starred, the way
/// `fts_match` in the `recall` script builds them.
fn search(conn: &rusqlite::Connection, terms: &[&str]) -> Vec<String> {
    let matcher = terms
        .iter()
        .map(|t| format!("\"{t}\"*"))
        .collect::<Vec<_>>()
        .join(" OR ");
    let out = dispatch(
        conn,
        &json!({"operation":"search","table":"facts","columns":["id"],"match":matcher}),
    )
    .unwrap();
    assert_eq!(out.error_code, None, "search must not error");
    out.payload
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_string())
        .collect()
}

/// Issue #14 verbatim: the fact was minted in the singular, the question is
/// asked in the plural, and before this package the keyword leg returned
/// nothing at all.
#[test]
fn the_plural_question_finds_the_singular_fact() {
    let conn = facts_store(&[
        ("f1", "hat Lieblingseditor", "Helix"),
        ("f2", "wohnt in", "Berlin"),
    ]);
    assert_eq!(
        search(&conn, &["lieblingseditoren"]),
        vec!["f1"],
        "the plural question reaches the singular predicate"
    );
    // and the direction prefix matching already covered does not regress
    assert_eq!(search(&conn, &["lieblingseditor"]), vec!["f1"]);
}

/// The mirror image, which is what scenario T8 pinned before this package: the
/// singular question against a plural predicate. It must keep working — the
/// fold replaces the prefix trick, it does not undo it.
#[test]
fn the_singular_question_still_finds_the_plural_fact() {
    let conn = facts_store(&[("f1", "hat Lieblingseditoren", "Helix")]);
    assert_eq!(search(&conn, &["lieblingseditor"]), vec!["f1"]);
    assert_eq!(search(&conn, &["lieblingseditoren"]), vec!["f1"]);
}

/// German inflection classes across both columns the keyword leg reads.
#[test]
fn german_inflection_meets_in_both_directions() {
    let conn = facts_store(&[
        ("f1", "hat Katze", "eine graue Katze"),
        ("f2", "arbeitet an", "drei Tage pro Woche"),
        ("f3", "hat Kind", "ein Kind in Berlin"),
    ]);
    assert_eq!(search(&conn, &["katzen"]), vec!["f1"]);
    assert_eq!(search(&conn, &["tag"]), vec!["f2"]);
    assert_eq!(search(&conn, &["kindern"]), vec!["f3"]);
}

/// The same for English, which the store sees just as often (the canonical
/// predicates of ruling Q2 are English snake_case).
#[test]
fn english_inflection_meets_in_both_directions() {
    let conn = facts_store(&[
        ("f1", "favorite_editor", "uses Helix as the editor"),
        ("f2", "owns", "two boxes of vinyl"),
        ("f3", "operates", "one server in Berlin"),
    ]);
    assert_eq!(search(&conn, &["editors"]), vec!["f1"]);
    assert_eq!(search(&conn, &["box"]), vec!["f2"]);
    assert_eq!(search(&conn, &["servers"]), vec!["f3"]);
}

/// Entity fidelity (ruling Q2), where it actually lives: the stemmer touches
/// the INDEX, never the row. An invented village and an invented company must
/// come back out byte-identical — and must still be findable by their own name,
/// because both sides of the search go through the same fold.
///
/// This is the honest form of "proper names are not stemmed": no tokenizer can
/// tell a village from a plural, so the promise the epic makes is about the
/// stored bytes, and it is kept.
#[test]
fn an_entity_name_survives_the_index_byte_identical_and_stays_findable() {
    let conn = facts_store(&[
        ("f1", "wohnt in", "Elvese"),
        ("f2", "arbeitet bei", "Sonnenhof Gartenbau"),
    ]);
    assert_eq!(search(&conn, &["Elvese"]), vec!["f1"], "found by its name");
    assert_eq!(search(&conn, &["Sonnenhof"]), vec!["f2"]);

    let out = dispatch(
        &conn,
        &json!({"operation":"select","table":"facts","columns":["id","claim"],
                "order_by":[{"col":"id","dir":"asc"}]}),
    )
    .unwrap();
    let rows = out.payload.as_array().unwrap();
    assert_eq!(rows[0]["claim"], "Elvese", "not one byte moved");
    assert_eq!(rows[1]["claim"], "Sonnenhof Gartenbau");
}

/// The counter-direction of the fold: an identifier is exactly what the keyword
/// leg exists for (scenario K1), and it must survive whole.
#[test]
fn an_identifier_is_not_folded() {
    let conn = facts_store(&[
        ("f1", "status", "invoice INV-2024-0815 is paid"),
        ("f2", "status", "invoice INV-2024-0816 is open"),
    ]);
    assert_eq!(search(&conn, &["INV-2024-0815"]), vec!["f1"]);
}

/// A fold that answers everything answers nothing. The minimum stem length is
/// what stops that, and it is observable here: `see` is three characters, so no
/// rule may touch it, and `seen` stays a different word.
///
/// Note what this test can and cannot show. The recall lane appends a prefix
/// star, so an over-folded QUERY token only ever widens the match — the
/// separation the `-s` guard buys sits on the INDEX side and is read off the
/// index itself in `store::query::fts_tokenizer`
/// (`the_index_holds_the_folded_terms_and_nothing_else`). At this level the
/// honest assertion is the one below.
#[test]
fn a_word_below_the_minimum_stem_length_is_not_folded() {
    let conn = facts_store(&[
        ("f1", "wohnt in", "ein Haus am See"),
        ("f2", "besitzt", "einen Atlas"),
    ]);
    assert!(
        search(&conn, &["seen"]).is_empty(),
        "`see` is too short to fold, so `seen` is simply another word"
    );
    assert_eq!(search(&conn, &["Haus"]), vec!["f1"]);
    assert_eq!(search(&conn, &["Atlas"]), vec!["f2"]);
}

/// The write path stays in sync: a row inserted through the op after the index
/// exists is folded by the triggers, not only by a rebuild.
#[test]
fn a_row_written_after_the_index_is_folded_by_its_triggers() {
    let conn = facts_store(&[("f1", "hat Lieblingseditor", "Helix")]);
    dispatch(
        &conn,
        &json!({"operation":"insert","table":"facts",
                "row":{"id":"f2","predicate":"hat Katze",
                       "canonical_predicate":"hat Katze","claim":"Minka"}}),
    )
    .unwrap();
    assert_eq!(search(&conn, &["katzen"]), vec!["f2"]);
    // …and an update moves the index with it (the delete+insert trigger pair)
    dispatch(
        &conn,
        &json!({"operation":"update","table":"facts",
                "set":{"canonical_predicate":"hat Hund","predicate":"hat Hund"},
                "where":{"id":"f2"}}),
    )
    .unwrap();
    assert_eq!(search(&conn, &["hunde"]), vec!["f2"]);
    assert!(
        search(&conn, &["katzen"]).is_empty(),
        "the stale folded term must be gone"
    );
}
